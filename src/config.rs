use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use std::path::PathBuf;

use crate::models::{DetailLevel, DocConfig, GroupBy, OperationRef, OperationSelector, SortMethod};

#[derive(Parser, Debug)]
#[command(name = "vimanam", version)]
#[command(about = "OpenAPI to Markdown documentation generator", long_about = None)]
// `input` is required for the conversion pipeline but must not be demanded when
// a subcommand runs (`vimanam completions zsh`). Subcommands are not arguments,
// so `required_unless_present` cannot name one; clap's idiom is to keep the
// positional `required` and let the subcommand negate that requirement.
// Conversion flags are meaningless alongside a subcommand, so they conflict.
#[command(subcommand_negates_reqs = true, args_conflicts_with_subcommands = true)]
#[command(group(ArgGroup::new("tree_output").args(["split", "output_mode"])))]
pub struct Cli {
    /// Path to the OpenAPI JSON or YAML file
    #[arg(value_name = "FILE", required = true)]
    pub input: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Output file path, or directory for split/skill output
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Write linked Markdown pages to the directory given by --output
    #[arg(long, value_enum, requires = "output", conflicts_with_all = ["stats", "max_tokens", "inline_schemas"])]
    pub split: Option<SplitArg>,

    /// Write an agent-navigable SKILL.md tree to the --output directory
    #[arg(long, value_enum, requires = "output", conflicts_with_all = ["stats", "max_tokens", "inline_schemas"])]
    pub output_mode: Option<OutputModeArg>,

    /// Estimated token budget for the compact split index or SKILL.md only; full navigation
    /// and detail pages remain available, without detail fallback
    #[arg(long, value_name = "N", requires = "tree_output")]
    pub overview_max_tokens: Option<usize>,

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

    /// Render exactly this operation, as "METHOD /path/template" (repeatable).
    /// The path must equal the spec's path template byte for byte (no
    /// substring, prefix or trailing-slash matching; Swagger 2 `basePath` is
    /// not part of it)
    #[arg(long = "operation", value_name = "METHOD PATH", value_parser = parse_operation)]
    pub operations: Vec<OperationRef>,

    /// Render exactly the operation(s) with this operationId (repeatable,
    /// case-sensitive)
    #[arg(long = "operation-id", value_name = "ID")]
    pub operation_ids: Vec<String>,

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

    /// Maximum schema traversal edges from a root (properties/items/composition/$ref).
    /// Zero retains only roots. Without selectors requires full detail with schemas.
    #[arg(long, value_name = "N")]
    pub schema_depth: Option<usize>,

    /// Read a named schema directly (repeatable); independent of endpoint reachability.
    /// Always renders full schema metadata, regardless of --detail.
    #[arg(long = "schema", value_name = "NAME", conflicts_with_all = ["split", "output_mode", "stats"])]
    pub schema_names: Vec<String>,

    /// Read a schema subtree and its ancestors, as NAME#/properties/FIELD (repeatable).
    /// JSON-pointer escaping applies. Referenced schemas may be traversed by the pointer.
    #[arg(long = "schema-field", value_name = "NAME#POINTER", conflicts_with_all = ["split", "output_mode", "stats"])]
    pub schema_fields: Vec<String>,

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

    /// Dry run: instead of Markdown, print a token-cost analysis (chars/4
    /// estimates over rendered output) at the configured detail level,
    /// filters and schema mode: per-endpoint slice costs with their share of
    /// TOTAL, per-schema definition and modeled inline-expansion costs with
    /// reference amplification, and hotspot rankings. Endpoint rows overlap
    /// through shared schemas and the document frame, so they do not sum to
    /// TOTAL. The hygiene report is never included
    #[arg(long, conflicts_with_all = ["output", "max_tokens", "stats", "split", "output_mode", "schema_names", "schema_fields"])]
    pub costs: bool,
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

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum, Debug)]
pub enum SplitArg {
    Service,
    Tag,
    Endpoint,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum, Debug)]
pub enum OutputModeArg {
    Skill,
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
        operation_selector: if cli.operations.is_empty() && cli.operation_ids.is_empty() {
            None
        } else {
            Some(OperationSelector {
                operations: cli.operations.iter().cloned().collect(),
                operation_ids: cli.operation_ids.iter().cloned().collect(),
            })
        },
        exclude_deprecated: cli.exclude_deprecated,
        required_only: cli.required_only,
        detail_level: if cli.schema_names.is_empty() && cli.schema_fields.is_empty() {
            cli.detail.into()
        } else {
            DetailLevel::Full
        },
        include_schemas: cli.include_schemas
            || !cli.schema_names.is_empty()
            || !cli.schema_fields.is_empty(),
        inline_schemas: cli.inline_schemas,
        schema_depth: cli.schema_depth,
        schema_names: cli.schema_names.clone(),
        schema_fields: cli.schema_fields.clone(),
        source_path: cli
            .input
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        include_examples: cli.include_examples,
        include_auth: cli.include_auth,
        // `--toc`/`--no-toc` override each other (last one wins), so at most
        // one of the pair is set; the TOC stays on unless --no-toc survives.
        include_toc: cli.toc || !cli.no_toc,
        sort_method: cli.sort.into(),
        max_tokens: cli.max_tokens,
        include_report: !cli.no_report
            && cli.schema_names.is_empty()
            && cli.schema_fields.is_empty(),
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

/// Parses an `--operation` value: a method and a path separated by the first
/// run of whitespace. The method is uppercased (as `--method-filter` does); the
/// path is kept verbatim and must start with `/`. A usage error (exit 2) names
/// the expected form.
fn parse_operation(value: &str) -> Result<OperationRef, String> {
    let usage =
        || format!("expected \"METHOD /path\" (for example \"GET /users/{{id}}\"), got {value:?}");
    let (method, rest) = value.split_once(char::is_whitespace).ok_or_else(usage)?;
    let path = rest.trim_start();
    if method.is_empty()
        || !method.chars().all(|c| c.is_ascii_alphabetic())
        || !path.starts_with('/')
    {
        return Err(usage());
    }
    Ok(OperationRef {
        method: method.to_ascii_uppercase(),
        path: path.to_string(),
    })
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
            split: None,
            output_mode: None,
            overview_max_tokens: None,
            method: false,
            group_by: GroupByArg::Service,
            flat: false,
            service_filter: None,
            path_filter: None,
            method_filter: None,
            operations: Vec::new(),
            operation_ids: Vec::new(),
            exclude_deprecated: false,
            required_only: false,
            detail: DetailLevelArg::Summary,
            include_schemas: false,
            inline_schemas: false,
            schema_depth: None,
            schema_names: Vec::new(),
            schema_fields: Vec::new(),
            include_examples: false,
            include_auth: false,
            toc: false,
            no_toc: false,
            sort: SortArg::Alpha,
            max_tokens: None,
            no_report: false,
            stats: false,
            costs: false,
        };

        let config = build_config(&cli);
        assert_matches!(config.detail_level, DetailLevel::Summary);

        let mut cli_basic = cli;
        cli_basic.detail = DetailLevelArg::Basic;
        let config_basic = build_config(&cli_basic);
        assert_matches!(config_basic.detail_level, DetailLevel::Basic);
    }

    #[test]
    fn parse_operation_splits_on_first_whitespace_run() {
        let op = parse_operation("get \t /users/{id}").unwrap();
        assert_eq!(op.method, "GET");
        assert_eq!(op.path, "/users/{id}");
        // Everything after the first whitespace run is the path, verbatim.
        let op = parse_operation("POST /a,b c").unwrap();
        assert_eq!(op.path, "/a,b c");
    }

    #[test]
    fn parse_operation_rejects_malformed_values() {
        for bad in ["GET", "/users", "GET users", " GET /users", "", "G3T /x"] {
            assert!(parse_operation(bad).is_err(), "{bad:?} should be rejected");
        }
    }
}
