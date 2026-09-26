use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use std::path::PathBuf;

use crate::models::{DetailLevel, DocConfig, GroupBy, SortMethod};

#[derive(Parser, Debug)]
#[command(name = "vimanam", version)]
#[command(about = "OpenAPI to Markdown documentation generator", long_about = None)]
// `input` is required for the conversion pipeline but must not be demanded when
// a subcommand runs (`vimanam completions zsh`). Subcommands are not arguments,
// so `required_unless_present` cannot name one; clap's idiom is to keep the
// positional `required` and let the subcommand negate that requirement.
// Conversion flags are meaningless alongside a subcommand, so they conflict.
#[command(subcommand_negates_reqs = true, args_conflicts_with_subcommands = true)]
pub struct Cli {
    /// Path to the OpenAPI JSON file
    #[arg(value_name = "FILE", required = true)]
    pub input: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Output file path
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Group endpoints by HTTP method instead of by service
    #[arg(long)]
    pub method: bool,

    /// Grouping method for endpoints
    #[arg(long, value_enum, default_value = "service")]
    pub group_by: GroupByArg,

    /// Generate a flat list without hierarchical structure
    #[arg(long)]
    pub flat: bool,

    /// Include only specific services (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub service_filter: Option<Vec<String>>,

    /// Filter endpoints by path pattern
    #[arg(long)]
    pub path_filter: Option<String>,

    /// Filter by HTTP methods (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub method_filter: Option<Vec<String>>,

    /// Hide deprecated endpoints
    #[arg(long)]
    pub exclude_deprecated: bool,

    /// Only show required parameters
    #[arg(long)]
    pub required_only: bool,

    /// Control amount of information
    #[arg(long, value_enum, default_value = "summary")]
    pub detail: DetailLevelArg,

    /// Include request/response schemas
    #[arg(long)]
    pub include_schemas: bool,

    /// Fully inline every `$ref` schema at each use site instead of linking to a
    /// shared "Schema Definitions" section (larger, self-contained output)
    #[arg(long)]
    pub inline_schemas: bool,

    /// Include request/response examples
    #[arg(long)]
    pub include_examples: bool,

    /// Show authentication requirements
    #[arg(long)]
    pub include_auth: bool,

    /// Include the table of contents (the default; when both are given,
    /// the later of --toc/--no-toc wins)
    #[arg(long, overrides_with = "no_toc")]
    pub toc: bool,

    /// Skip table of contents
    #[arg(long)]
    pub no_toc: bool,

    /// Sorting method
    #[arg(long, value_enum, default_value = "alpha")]
    pub sort: SortArg,

    /// Fit output to a token budget, stepping detail down (full → summary) as
    /// needed; what was trimmed is reported on stderr. The budget covers the
    /// documentation body only: the spec hygiene report is still appended
    /// outside it (add --no-report to drop it)
    #[arg(long, value_name = "N")]
    pub max_tokens: Option<usize>,

    /// Skip the spec hygiene report appended after the documentation
    #[arg(long)]
    pub no_report: bool,

    /// Dry run: instead of Markdown, print a plain-text table of visible
    /// endpoints and estimated tokens (chars/4) per service at the configured
    /// detail level and filters, to size slices before choosing
    /// --service-filter/--detail/--max-tokens. Each row is a render of that
    /// service alone; TOTAL is one render of the whole document, so it is not
    /// necessarily the sum of the rows. The hygiene report is never included
    #[arg(long, conflicts_with_all = ["output", "max_tokens"])]
    pub stats: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Generate shell completions and print them to stdout
    ///
    /// Example: `vimanam completions zsh > ~/.zfunc/_vimanam`
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Compare two versions of a spec and report what changed, classified as
    /// breaking, non-breaking or needing review
    ///
    /// Endpoints are matched by method and path, parameters by name and
    /// location, responses by status code. Request and response bodies are
    /// compared as fully resolved schemas, so a change to a shared component
    /// schema is reported on every endpoint that references it. Only the
    /// first media type of a body or response is compared.
    ///
    /// Example: `vimanam diff v1/openapi.json v2/openapi.json --fail-on-breaking`
    Diff(DiffArgs),
}

/// Arguments of the `diff` subcommand. It has its own `-o/--output` because
/// the top-level conversion flags conflict with subcommands.
#[derive(Args, Debug)]
pub struct DiffArgs {
    /// The older spec (JSON or YAML)
    #[arg(value_name = "OLD")]
    pub old: PathBuf,

    /// The newer spec (JSON or YAML)
    #[arg(value_name = "NEW")]
    pub new: PathBuf,

    /// Append a Deltas section: spec hygiene counts for both specs and the
    /// estimated token size of each at --detail full --include-schemas
    #[arg(long)]
    pub report: bool,

    /// Output format: a Markdown report or machine-readable JSON with stable
    /// change IDs
    #[arg(long, value_enum, default_value_t = DiffFormatArg::Markdown)]
    pub format: DiffFormatArg,

    /// Exit with status 3 when any breaking change is found, after writing the
    /// full report
    ///
    /// Exit codes: 0 no breaking changes (changes needing review do not
    /// count), 1 a spec failed to parse or the output could not be written,
    /// 2 usage error, 3 breaking changes found
    #[arg(long)]
    pub fail_on_breaking: bool,

    /// Write the diff to FILE instead of stdout
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

/// Output format of the `diff` subcommand.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum DiffFormatArg {
    /// Human-readable Markdown report
    Markdown,
    /// Machine-readable JSON with a stable, content-derived ID per change
    Json,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum GroupByArg {
    Service,
    Method,
    Path,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum DetailLevelArg {
    Summary,
    Basic,
    Standard,
    Full,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum SortArg {
    Alpha,
    PathLength,
    None,
}

impl From<GroupByArg> for GroupBy {
    fn from(arg: GroupByArg) -> Self {
        match arg {
            GroupByArg::Service => GroupBy::Service,
            GroupByArg::Method => GroupBy::Method,
            GroupByArg::Path => GroupBy::Path,
        }
    }
}

impl From<DetailLevelArg> for DetailLevel {
    fn from(arg: DetailLevelArg) -> Self {
        match arg {
            DetailLevelArg::Summary => DetailLevel::Summary,
            DetailLevelArg::Basic => DetailLevel::Basic,
            DetailLevelArg::Standard => DetailLevel::Standard,
            DetailLevelArg::Full => DetailLevel::Full,
        }
    }
}

impl From<SortArg> for SortMethod {
    fn from(arg: SortArg) -> Self {
        match arg {
            SortArg::Alpha => SortMethod::Alphabetical,
            SortArg::PathLength => SortMethod::PathLength,
            SortArg::None => SortMethod::None,
        }
    }
}

/// Converts parsed CLI arguments into the internal [`DocConfig`].
/// Grouping precedence: `--flat` > `--method` > `--group-by` > default (service).
pub fn build_config(cli: &Cli) -> DocConfig {
    // Determine grouping method.
    // `--group-by` always has a value (clap default), so it serves as the
    // base; `--flat` and `--method` are higher-precedence overrides.
    let group_by = if cli.flat {
        GroupBy::Flat
    } else if cli.method {
        GroupBy::Method
    } else {
        cli.group_by.into()
    };

    let config = DocConfig {
        group_by,
        service_filter: cli.service_filter.clone(),
        path_filter: cli.path_filter.clone(),
        // HTTP methods are stored uppercase on each endpoint, so normalize the
        // filter values too — otherwise `--method-filter get` matches nothing.
        method_filter: cli
            .method_filter
            .as_ref()
            .map(|methods| methods.iter().map(|m| m.to_uppercase()).collect()),
        exclude_deprecated: cli.exclude_deprecated,
        required_only: cli.required_only,
        detail_level: cli.detail.into(),
        include_schemas: cli.include_schemas,
        inline_schemas: cli.inline_schemas,
        include_examples: cli.include_examples,
        include_auth: cli.include_auth,
        // `--toc`/`--no-toc` override each other (last one wins), so at most
        // one of the pair is set; the TOC stays on unless --no-toc survives.
        include_toc: cli.toc || !cli.no_toc,
        sort_method: cli.sort.into(),
        max_tokens: cli.max_tokens,
        include_report: !cli.no_report,
    };

    // Warn if --include-schemas or --include-examples is set but detail is not
    // Full. The current level is reported in the same lowercase spelling the
    // user types (`--detail standard`), not the Debug-derived `Standard`.
    let detail_name = detail_arg_name(cli.detail);
    if config.include_schemas && config.detail_level != DetailLevel::Full {
        eprintln!(
            "vimanam: --include-schemas has no effect at --detail {detail_name}; use --detail full."
        );
    }
    if config.include_examples && config.detail_level != DetailLevel::Full {
        eprintln!(
            "vimanam: --include-examples has no effect at --detail {detail_name}; use --detail full."
        );
    }
    // --inline-schemas only changes how schemas render, so it does nothing
    // without --include-schemas.
    if config.inline_schemas && !config.include_schemas {
        eprintln!("vimanam: --inline-schemas has no effect without --include-schemas.");
    }
    // --required-only only filters the parameters table, which is rendered at
    // --detail standard and full.
    if config.required_only
        && matches!(
            config.detail_level,
            DetailLevel::Basic | DetailLevel::Summary
        )
    {
        eprintln!(
            "vimanam: --required-only has no effect at --detail {detail_name}; use --detail standard or full."
        );
    }

    config
}

/// The `--detail` value name as the user spells it (e.g. `standard`), for stderr
/// messages. Matches the kebab-case names clap derives for [`DetailLevelArg`].
fn detail_arg_name(detail: DetailLevelArg) -> &'static str {
    match detail {
        DetailLevelArg::Summary => "summary",
        DetailLevelArg::Basic => "basic",
        DetailLevelArg::Standard => "standard",
        DetailLevelArg::Full => "full",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::assert_matches;

    #[test]
    fn test_detail_level_conversion() {
        let cli = Cli {
            input: Some(PathBuf::from("spec.json")),
            command: None,
            output: None,
            method: false,
            group_by: GroupByArg::Service,
            flat: false,
            service_filter: None,
            path_filter: None,
            method_filter: None,
            exclude_deprecated: false,
            required_only: false,
            detail: DetailLevelArg::Summary,
            include_schemas: false,
            inline_schemas: false,
            include_examples: false,
            include_auth: false,
            toc: false,
            no_toc: false,
            sort: SortArg::Alpha,
            max_tokens: None,
            no_report: false,
            stats: false,
        };

        let config = build_config(&cli);
        assert_matches!(config.detail_level, DetailLevel::Summary);

        let mut cli_basic = cli;
        cli_basic.detail = DetailLevelArg::Basic;
        let config_basic = build_config(&cli_basic);
        assert_matches!(config_basic.detail_level, DetailLevel::Basic);
    }
}
