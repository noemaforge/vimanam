mod config;
mod diff;
mod diff_json;
mod markdown;
mod models;
mod parser;
mod report;
mod selection;
mod stats;
mod utils;

use std::fs::{self, File};
use std::io::{BufWriter, Write, stdout};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use log::info;

use crate::config::{Cli, Commands, DiffArgs, DiffFormatArg, build_config};
use crate::markdown::generate_markdown;
use crate::models::{ApiDocumentation, DocConfig};
use crate::parser::{parse_openapi, parse_openapi_bytes};

/// Writes the documentation and, unless `--no-report` was given, the spec
/// hygiene report after it. Both the file and stdout paths go through here so
/// they can't drift apart.
fn write_output<W: Write>(
    writer: &mut W,
    api_doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    if !config.include_report {
        return generate_markdown(writer, api_doc, config).context("Failed to generate markdown");
    }

    // The views end with differing trailing whitespace (one newline at
    // `--detail summary`, a blank line otherwise). Render the body into a
    // buffer and normalize it to exactly one trailing newline so the report's
    // rule is always preceded by exactly one blank line. Only this path
    // buffers: `--no-report` output stays byte-identical to the views' own.
    let mut body = Vec::new();
    generate_markdown(&mut body, api_doc, config).context("Failed to generate markdown")?;
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

/// Exit status of `diff --fail-on-breaking` when a breaking change was found.
/// Distinct from 1 (runtime error) and 2 (usage error) so CI can tell "the
/// spec is incompatible" from "the spec failed to parse".
const EXIT_BREAKING_CHANGES: u8 = 3;

/// Runs `vimanam diff <OLD> <NEW>`: parses both specs, writes the comparison
/// (with deltas under `--report`) to the file or stdout, and returns exit
/// status 3 under `--fail-on-breaking` when a breaking change was found. The
/// full report is always written first.
fn run_diff(args: &DiffArgs) -> Result<ExitCode> {
    // Each input is read once and parsed from the exact bytes that get hashed,
    // so `file_sha256` can never refer to different contents than the diff.
    let old_bytes = fs::read(&args.old)
        .with_context(|| format!("Failed to parse OpenAPI file: {:?}", args.old))?;
    let new_bytes = fs::read(&args.new)
        .with_context(|| format!("Failed to parse OpenAPI file: {:?}", args.new))?;
    let old_sha = diff_json::sha256_hex(&old_bytes);
    let new_sha = diff_json::sha256_hex(&new_bytes);

    let old = parse_openapi_bytes(&old_bytes, &file_extension(&args.old), Some(&args.old))
        .with_context(|| format!("Failed to parse OpenAPI file: {:?}", args.old))?;
    let new = parse_openapi_bytes(&new_bytes, &file_extension(&args.new), Some(&args.new))
        .with_context(|| format!("Failed to parse OpenAPI file: {:?}", args.new))?;

    let spec_diff = diff::diff(&old, &new);
    let deltas = if args.report {
        Some(diff::compute_deltas(&old, &new).context("Failed to compute deltas")?)
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
            let document = diff_json::to_json(&spec_diff, deltas.as_ref(), &old_sha, &new_sha);
            let mut writer = BufWriter::new(create_output_file(output_path)?);
            write_json_diff(&mut writer, &document)?;
            info!("Diff written to: {:?}", output_path);
        }
        (None, DiffFormatArg::Markdown) => {
            diff::write_diff(&mut stdout(), &spec_diff, deltas.as_ref())
                .context("Failed to write diff")?;
            stdout().flush().context("Failed to write diff")?;
        }
        (None, DiffFormatArg::Json) => {
            let document = diff_json::to_json(&spec_diff, deltas.as_ref(), &old_sha, &new_sha);
            write_json_diff(&mut stdout(), &document)?;
        }
    }

    // The complete document is written and flushed before the exit code is
    // decided, so exit 3 never truncates the report.
    if args.fail_on_breaking && spec_diff.has_breaking() {
        return Ok(ExitCode::from(EXIT_BREAKING_CHANGES));
    }
    Ok(ExitCode::SUCCESS)
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
fn write_json_diff<W: Write>(writer: &mut W, document: &diff_json::JsonDiff) -> Result<()> {
    serde_json::to_writer_pretty(&mut *writer, document).context("Failed to write diff")?;
    writer.write_all(b"\n").context("Failed to write diff")?;
    writer.flush().context("Failed to write diff")
}

/// Warns on stderr about `--operation`/`--operation-id` values whose
/// operations exist in the spec but were all removed by another filter.
fn warn_filtered_out_selectors(api_doc: &ApiDocumentation, config: &DocConfig) {
    for warning in selection::filtered_out_warnings(api_doc, config) {
        eprintln!("{warning}");
    }
}

fn create_output_file(path: &Path) -> Result<File> {
    File::create(path).with_context(|| format!("Failed to create output file: {:?}", path))
}

/// Parses CLI arguments, parses the spec, and writes markdown to the
/// requested output (file or stdout). Returns the process exit status for
/// the success paths; errors are mapped to status 1 by `main`.
fn run() -> Result<ExitCode> {
    // Initialize logger
    env_logger::init();

    // Parse command-line arguments
    let cli = Cli::parse();

    // Subcommands short-circuit the conversion pipeline. The match is
    // exhaustive so a new `Commands` variant fails to compile here instead of
    // falling through to the "missing input file" error below.
    match &cli.command {
        Some(Commands::Completions { shell }) => {
            clap_complete::generate(*shell, &mut Cli::command(), "vimanam", &mut stdout());
            return Ok(ExitCode::SUCCESS);
        }
        Some(Commands::Diff(args)) => return run_diff(args),
        None => {}
    }

    // clap marks `input` required (negated only by a subcommand), so it is
    // always set once no subcommand was given; the bail is a defensive guard.
    let Some(input) = &cli.input else {
        bail!("missing input file; run `vimanam --help` for usage");
    };

    // Build configuration
    let config = build_config(&cli);

    // Parse OpenAPI spec
    let api_doc =
        parse_openapi(input).with_context(|| format!("Failed to parse OpenAPI file: {input:?}"))?;

    // A selector naming no operation in the spec is a mistake, not an empty
    // document: fail before any output (or output file) is produced.
    if let Some(selector) = &config.operation_selector
        && let Some(message) = selection::unmatched_selectors(&api_doc, selector)
    {
        bail!(message);
    }

    // `--stats` is a dry run: print the per-service size table to stdout
    // instead of the documentation. clap rejects `-o` and `--max-tokens`
    // alongside it, and the hygiene report is never emitted in this mode.
    if cli.stats {
        let stats = stats::compute(&api_doc, &config).context("Failed to compute stats")?;
        stats::write_stats(&mut stdout(), &stats).context("Failed to write stats")?;
        warn_filtered_out_selectors(&api_doc, &config);
        return Ok(ExitCode::SUCCESS);
    }

    // Generate markdown
    if let Some(split) = cli.split {
        markdown::split::write_tree(
            cli.output.as_ref().expect("clap requires --output"),
            input,
            &api_doc,
            &config,
            split,
            cli.overview_max_tokens,
        )?;
        warn_filtered_out_selectors(&api_doc, &config);
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(output_path) = &cli.output {
        // Write to file
        let mut writer = BufWriter::new(create_output_file(output_path)?);

        write_output(&mut writer, &api_doc, &config)?;

        info!("Documentation written to: {:?}", output_path);
    } else {
        // Write to stdout
        let mut writer = stdout();

        write_output(&mut writer, &api_doc, &config)?;
    }
    warn_filtered_out_selectors(&api_doc, &config);

    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Error: {:#}", err);
            ExitCode::from(1)
        }
    }
}
