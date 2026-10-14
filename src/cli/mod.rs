//! Optional command-line adapter. Output and notices are supplied by the caller.

mod config;
mod diff_inputs;
use crate::{costs, diff, markdown, parser, report, selection, stats};
pub use config::Cli;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use log::info;

use self::config::{Commands, DiffArgs, DiffFormatArg, build_config};
use crate::markdown::generate_markdown_with_notices;
use crate::models::{ApiDocumentation, DocConfig};
use parser::{parse_openapi, parse_openapi_bytes};

/// Writes the documentation and, unless `--no-report` was given, the spec
/// hygiene report after it. Both the file and stdout paths go through here so
/// they can't drift apart.
fn write_output<W: Write>(
    writer: &mut W,
    api_doc: &ApiDocumentation,
    config: &DocConfig,
    notice: &mut dyn FnMut(&str),
) -> Result<()> {
    if !config.include_report {
        return generate_markdown_with_notices(writer, api_doc, config, notice)
            .context("Failed to generate markdown");
    }

    // The views end with differing trailing whitespace (one newline at
    // `--detail summary`, a blank line otherwise). Render the body into a
    // buffer and normalize it to exactly one trailing newline so the report's
    // rule is always preceded by exactly one blank line. Only this path
    // buffers: `--no-report` output stays byte-identical to the views' own.
    let mut body = Vec::new();
    generate_markdown_with_notices(&mut body, api_doc, config, notice)
        .context("Failed to generate markdown")?;
    while body.last() == Some(&b'\n') {
        body.pop();
    }
    body.push(b'\n');
    writer
        .write_all(&body)
        .context("Failed to write documentation")?;

    let hygiene = report::analyze(api_doc, config);
    report::write_report(writer, &hygiene).context("Failed to write spec hygiene report")?;

    Ok(())
}

/// Runs file-pair or Git-ref `vimanam diff`: parses both specs, writes the comparison
/// (with deltas under `--report`) to the file or stdout, and returns exit
/// status 3 under `--fail-on-breaking` when a breaking change was found. The
/// full report is always written first.
fn run_diff<W: Write>(
    args: &DiffArgs,
    output: &mut W,
    notice: &mut dyn FnMut(&str),
) -> Result<bool> {
    // Each input is read once and parsed from the exact bytes that get hashed,
    // so `file_sha256` can never refer to different contents than the diff.
    let (old_input, new_input) = diff_inputs::read(args)?;
    let old_sha = diff::json::sha256_hex(&old_input.bytes);
    let new_sha = diff::json::sha256_hex(&new_input.bytes);

    if old_input.is_graphql() || new_input.is_graphql() {
        anyhow::ensure!(
            old_input.is_graphql() && new_input.is_graphql(),
            "Cannot compare different contract formats"
        );
        return run_graphql_diff(
            args, &old_input, &new_input, &old_sha, &new_sha, output, notice,
        );
    }
    anyhow::ensure!(
        !args.context,
        "--context is GraphQL-only; OpenAPI and protobuf diffs do not support it"
    );

    if old_input.is_protobuf() || new_input.is_protobuf() {
        anyhow::ensure!(
            old_input.is_protobuf() && new_input.is_protobuf(),
            "Cannot compare different contract formats"
        );
        anyhow::ensure!(
            args.contract == 2,
            "Protobuf requires --contract 2; --contract 1 is OpenAPI-only"
        );
        let old = old_input.protobuf(&args.proto_paths)?;
        let new = new_input.protobuf(&args.proto_paths)?;
        let changes = crate::contract::protobuf::diff(&old, &new)?;
        let breaking = changes.iter().any(|c| {
            c.severity == crate::contract::Severity::Breaking
                && match args.breaking {
                    config::BreakingArg::All => true,
                    config::BreakingArg::Wire => c.impacts.iter().any(|i| i == "wire"),
                    config::BreakingArg::Source => {
                        c.impacts.iter().any(|i| i == "source" || i == "wire_json")
                    }
                }
        });
        let document = crate::contract::json::to_json(
            crate::contract::protobuf::adapt(&old),
            crate::contract::protobuf::adapt(&new),
            changes,
            &old_sha,
            &new_sha,
            None,
        );
        let mut writer: Box<dyn Write + '_> = if let Some(path) = &args.output {
            Box::new(BufWriter::new(create_output_file(path)?))
        } else {
            Box::new(output)
        };
        match args.format {
            DiffFormatArg::Json => write_json_diff(&mut writer, &document)?,
            DiffFormatArg::Markdown => crate::contract::protobuf::write_diff(
                &mut writer,
                &document.changes,
                args.report,
                &old,
                &new,
            )?,
        };
        return Ok(args.fail_on_breaking && breaking);
    }

    let old = old_input.parse()?;
    let new = new_input.parse()?;

    let spec_diff = diff::diff(&old, &new);
    let deltas = if args.report {
        Some(diff::compute_deltas(&old, &new).context("Failed to compute deltas")?)
    } else {
        None
    };

    let neutral = if args.format == DiffFormatArg::Json && args.contract == 2 {
        Some(crate::contract::json::openapi(
            &old,
            &new,
            deltas.as_ref(),
            &old_sha,
            &new_sha,
        )?)
    } else {
        None
    };
    match (&args.output, args.format) {
        (Some(output_path), DiffFormatArg::Markdown) => {
            let mut writer = BufWriter::new(create_output_file(output_path)?);
            diff::write_diff(&mut writer, &spec_diff, deltas.as_ref())
                .context("Failed to write diff")?;
            writer.flush().context("Failed to write diff")?;
            info!("Diff written to: {:?}", output_path);
        }
        (Some(output_path), DiffFormatArg::Json) => {
            let document = diff::json::to_json(&spec_diff, deltas.as_ref(), &old_sha, &new_sha);
            let mut writer = BufWriter::new(create_output_file(output_path)?);
            if let Some(neutral) = &neutral {
                write_json_diff(&mut writer, neutral)?;
            } else {
                write_json_diff(&mut writer, &document)?;
            }
            info!("Diff written to: {:?}", output_path);
        }
        (None, DiffFormatArg::Markdown) => {
            diff::write_diff(output, &spec_diff, deltas.as_ref())
                .context("Failed to write diff")?;
            output.flush().context("Failed to write diff")?;
        }
        (None, DiffFormatArg::Json) => {
            let document = diff::json::to_json(&spec_diff, deltas.as_ref(), &old_sha, &new_sha);
            if let Some(neutral) = &neutral {
                write_json_diff(output, neutral)?;
            } else {
                write_json_diff(output, &document)?;
            }
        }
    }

    // The complete document is written and flushed before the exit code is
    // decided, so exit 3 never truncates the report.
    if args.fail_on_breaking
        && neutral
            .as_ref()
            .map_or_else(|| spec_diff.has_breaking(), |d| d.has_breaking())
    {
        return Ok(true);
    }
    Ok(false)
}

/// GraphQL SDL/introspection diff through the neutral v2 model. Every
/// argument is validated and both schemas parsed before output is created.
fn run_graphql_diff<W: Write>(
    args: &DiffArgs,
    old_input: &diff_inputs::Input,
    new_input: &diff_inputs::Input,
    old_sha: &str,
    new_sha: &str,
    output: &mut W,
    notice: &mut dyn FnMut(&str),
) -> Result<bool> {
    use crate::contract::graphql;
    anyhow::ensure!(
        args.contract == 2,
        "GraphQL requires --contract 2; --contract 1 is OpenAPI-only"
    );
    anyhow::ensure!(
        args.breaking == config::BreakingArg::All,
        "--breaking wire|source applies to protobuf only"
    );
    anyhow::ensure!(
        !(args.context && args.format == DiffFormatArg::Json),
        "--context renders Markdown; it cannot be combined with --format json"
    );
    let old = old_input.graphql()?;
    let new = new_input.graphql()?;
    let changes = graphql::diff(&old, &new)?;
    let document = crate::contract::json::to_json(
        graphql::adapt(&old),
        graphql::adapt(&new),
        changes,
        old_sha,
        new_sha,
        None,
    );
    let mut writer: Box<dyn Write + '_> = if let Some(path) = &args.output {
        Box::new(BufWriter::new(create_output_file(path)?))
    } else {
        Box::new(output)
    };
    match args.format {
        DiffFormatArg::Json => write_json_diff(&mut writer, &document)?,
        DiffFormatArg::Markdown => {
            graphql::write_diff(&mut writer, &document.changes, args.report, &old, &new)?;
            if args.context {
                writeln!(writer, "## Focused context\n")?;
                graphql::write_focused_context(
                    &mut writer,
                    &new,
                    &document.changes,
                    args.max_tokens,
                    notice,
                )?;
                writer.flush()?;
            }
        }
    }
    Ok(args.fail_on_breaking && document.has_breaking())
}

/// The raw file extension of `path`, used to pick the parser. Empty when the
/// name has none; `parse_openapi_bytes` handles the case-insensitivity.
fn file_extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Pretty-prints the JSON diff with a trailing newline, flushing before
/// returning so the exit-code check can never truncate the document.
fn write_json_diff<W: Write, T: serde::Serialize>(writer: &mut W, document: &T) -> Result<()> {
    serde_json::to_writer_pretty(&mut *writer, document).context("Failed to write diff")?;
    writer.write_all(b"\n").context("Failed to write diff")?;
    writer.flush().context("Failed to write diff")
}

/// Warns on stderr about `--operation`/`--operation-id` values whose
/// operations exist in the spec but were all removed by another filter.
fn warn_filtered_out_selectors(
    api_doc: &ApiDocumentation,
    config: &DocConfig,
    notice: &mut dyn FnMut(&str),
) {
    for warning in selection::filtered_out_warnings(api_doc, config) {
        notice(&warning);
    }
}

fn create_output_file(path: &Path) -> Result<File> {
    File::create(path).with_context(|| format!("Failed to create output file: {:?}", path))
}

/// Execute parsed CLI arguments; return whether breaking changes require exit 3.
/// Console output is written only to `output`; diagnostics go to `notice`.
pub fn run<W: Write>(cli: &Cli, output: &mut W, notice: &mut dyn FnMut(&str)) -> Result<bool> {
    // Subcommands short-circuit the conversion pipeline. The match is
    // exhaustive so a new `Commands` variant fails to compile here instead of
    // falling through to the "missing input file" error below.
    match &cli.command {
        Some(Commands::Completions { shell }) => {
            let mut completions = Vec::new();
            clap_complete::generate(*shell, &mut Cli::command(), "vimanam", &mut completions);
            output
                .write_all(&completions)
                .context("Failed to write shell completions")?;
            output
                .flush()
                .context("Failed to write shell completions")?;
            return Ok(false);
        }
        Some(Commands::Diff(args)) => return run_diff(args, output, notice),
        None => {}
    }

    // clap marks `input` required (negated only by a subcommand), so it is
    // always set once no subcommand was given; the bail is a defensive guard.
    let Some(input) = &cli.input else {
        bail!("missing input file; run `vimanam --help` for usage");
    };

    // Build configuration
    let config = build_config(cli, notice);

    // Parse OpenAPI spec
    let api_doc = if crate::contract::graphql::is_input(input) {
        crate::contract::graphql::documentation(
            &crate::contract::graphql::Document::read(input)
                .with_context(|| format!("Failed to parse GraphQL schema: {input:?}"))?,
        )
    } else if crate::contract::protobuf::is_input(input) {
        crate::contract::protobuf::documentation(&crate::contract::protobuf::Document::read(
            input,
            &cli.proto_paths,
        )?)
    } else {
        parse_openapi(input).with_context(|| format!("Failed to parse OpenAPI file: {input:?}"))?
    };

    // A selector naming no operation in the spec is a mistake, not an empty
    // document: fail before any output (or output file) is produced.
    selection::validate(&api_doc, &config)?;

    // `--stats` is a dry run: print the per-service size table to stdout
    // instead of the documentation. clap rejects `-o` and `--max-tokens`
    // alongside it, and the hygiene report is never emitted in this mode.
    if cli.stats {
        let stats = stats::compute(&api_doc, &config).context("Failed to compute stats")?;
        stats::write_stats(output, &stats).context("Failed to write stats")?;
        warn_filtered_out_selectors(&api_doc, &config, notice);
        return Ok(false);
    }

    // `--costs` is a dry run like `--stats`, with per-endpoint and per-schema
    // cost rows, reference amplification and hotspots (#47). It honors the
    // configured detail level, filters and schema-rendering mode, and never
    // emits the hygiene report.
    if cli.costs {
        let costs = costs::compute(&api_doc, &config).context("Failed to compute token costs")?;
        costs::write_costs(output, &costs).context("Failed to write token costs")?;
        warn_filtered_out_selectors(&api_doc, &config, notice);
        return Ok(false);
    }

    // Generate markdown
    let tree_layout = cli
        .split
        .map(|split| markdown::split::TreeLayout::Split(split.into()))
        .or_else(|| cli.output_mode.map(|_| markdown::split::TreeLayout::Skill));
    if let Some(layout) = tree_layout {
        markdown::split::write_tree(
            cli.output.as_ref().expect("clap requires --output"),
            input,
            &api_doc,
            &config,
            layout,
            cli.overview_max_tokens,
            notice,
        )?;
        warn_filtered_out_selectors(&api_doc, &config, notice);
        return Ok(false);
    }
    if let Some(output_path) = &cli.output {
        // Write to file
        let mut writer = BufWriter::new(create_output_file(output_path)?);

        write_output(&mut writer, &api_doc, &config, notice)?;

        info!("Documentation written to: {:?}", output_path);
    } else {
        // Write to stdout
        write_output(output, &api_doc, &config, notice)?;
    }
    warn_filtered_out_selectors(&api_doc, &config, notice);

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io;

    struct FailingWriter {
        bytes: Vec<u8>,
        fail_write: bool,
        fail_flush: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "write failed"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "flush failed"));
            }
            Ok(())
        }
    }

    fn completions_cli() -> Cli {
        Cli::try_parse_from(["vimanam", "completions", "bash"]).unwrap()
    }

    #[test]
    fn completions_return_write_errors() {
        let mut writer = FailingWriter {
            bytes: Vec::new(),
            fail_write: true,
            fail_flush: false,
        };
        let error = run(&completions_cli(), &mut writer, &mut |_| {}).unwrap_err();
        assert_eq!(error.to_string(), "Failed to write shell completions");
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn completions_return_flush_errors() {
        let mut writer = FailingWriter {
            bytes: Vec::new(),
            fail_write: false,
            fail_flush: true,
        };
        let error = run(&completions_cli(), &mut writer, &mut |_| {}).unwrap_err();
        assert_eq!(error.to_string(), "Failed to write shell completions");
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(!writer.bytes.is_empty());
    }

    #[test]
    fn completions_preserve_generated_bytes() {
        let cli = completions_cli();
        let mut expected = Vec::new();
        let Some(Commands::Completions { shell }) = &cli.command else {
            unreachable!("the parsed command is completions");
        };
        clap_complete::generate(*shell, &mut Cli::command(), "vimanam", &mut expected);
        let mut actual = Vec::new();
        assert!(!run(&cli, &mut actual, &mut |_| {}).unwrap());
        assert_eq!(actual, expected);
    }
}
