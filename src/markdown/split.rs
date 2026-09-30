//! Linked directory output. Rendering is independent of single-file budgets;
//! ownership is tracked by hashes so regeneration never discards user edits.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::SplitArg;
use crate::diff_json::sha256_hex;
use crate::models::{ApiDocumentation, DetailLevel, DocConfig, Endpoint};
use crate::utils::{clean_for_id, resolve_schema_reference};

use super::endpoint::write_endpoint;
use super::schema::{SchemaContext, short_schema_reference, write_schema_table};
use super::{detail_level_name, estimate_tokens, service_is_visible, visible_endpoints};

const MANIFEST: &str = ".vimanam-manifest.json";

pub(crate) enum TreeLayout {
    Split(SplitArg),
    Skill,
}

/// Escape untrusted spec text used in generated navigation (including headings).
pub(super) fn escape(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        match ch {
            '\n' | '\r' => result.push(' '),
            '\\' | '[' | ']' | '*' | '_' | '`' | '<' | '>' | '#' | '|' => {
                result.push('\\');
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    result
}

fn filename(label: &str, identity: &str) -> String {
    let slug: String = clean_for_id(label)
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .take(32)
        .collect();
    format!(
        "{}-{}.md",
        if slug.is_empty() { "item" } else { &slug },
        sha256_hex(identity.as_bytes())
    )
}

pub(super) fn schema_filename(reference: &str) -> String {
    filename(&short_schema_reference(reference), reference)
}

fn endpoint_identity(endpoint: &Endpoint) -> String {
    format!("{} {}", endpoint.method, endpoint.path)
}

fn endpoint_anchor(endpoint: &Endpoint) -> String {
    format!(
        "operation-{}",
        sha256_hex(endpoint_identity(endpoint).as_bytes())
    )
}

/// Skill names have stricter rules than Markdown heading IDs: ASCII hyphen
/// case, no adjacent separators, and at most 64 bytes including the suffix.
fn skill_name(title: &str) -> String {
    let mut base = String::new();
    let mut separator = false;
    for ch in title.chars() {
        if !ch.is_ascii_alphanumeric() {
            separator = !base.is_empty();
            continue;
        }
        if separator {
            base.push('-');
            separator = false;
        }
        base.push(ch.to_ascii_lowercase());
        if base.len() >= 60 {
            base.truncate(60);
            break;
        }
    }
    let base = base.trim_end_matches('-');
    if base.is_empty() {
        "api".to_string()
    } else {
        format!("{base}-api")
    }
}

fn quote_shell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn retrieval(input: &Path, split: SplitArg, endpoint: Option<&Endpoint>) -> String {
    let mode = match split {
        SplitArg::Service => "service",
        SplitArg::Tag => "tag",
        SplitArg::Endpoint => "endpoint",
    };
    let selection = endpoint
        .map(|ep| format!(" --operation {}", quote_shell(&endpoint_identity(ep))))
        .unwrap_or_default();
    format!(
        "vimanam {} --split {mode} -o ./vimanam-full --detail full --include-schemas --include-examples --include-auth{selection}",
        quote_shell(&input.to_string_lossy())
    )
}

fn entry(endpoint: &Endpoint, target: &str) -> String {
    let operation = escape(&endpoint_identity(endpoint));
    let id = endpoint
        .operation_id
        .as_ref()
        .map(|id| format!(" — {}", escape(id)))
        .unwrap_or_default();
    let summary = endpoint
        .summary
        .as_deref()
        .or(endpoint.description.as_deref())
        .unwrap_or("");
    // A navigation entry is intentionally brief even if the spec description is huge.
    let brief: String = summary.chars().take(160).collect();
    let suffix = if summary.chars().count() > 160 {
        "…"
    } else {
        ""
    };
    format!(
        "- [{operation}]({target}){id} — {}{suffix}\n",
        escape(&brief)
    )
}

fn page_notice(
    input: &Path,
    split: SplitArg,
    config: &DocConfig,
    endpoint: Option<&Endpoint>,
) -> String {
    let mut omitted = Vec::new();
    if config.detail_level != DetailLevel::Full {
        omitted.push("full parameters, schemas and examples");
    } else {
        if !config.include_schemas {
            omitted.push("schema tables");
        }
        if !config.include_examples {
            omitted.push("examples");
        }
    }
    if config.required_only {
        omitted.push("optional parameters");
    }
    if omitted.is_empty() {
        return String::new();
    }
    format!(
        "\n> This page omits {} under the requested options. Retrieve fuller detail (in a separate directory):\n\n```sh\n{}\n```\n\n",
        omitted.join(", "),
        retrieval(input, split, endpoint)
    )
}

/// Generate every page before touching disk, then preflight the entire tree.
pub(crate) fn write_tree(
    output: &Path,
    input: &Path,
    doc: &ApiDocumentation,
    config: &DocConfig,
    layout: TreeLayout,
    overview_budget: Option<usize>,
) -> Result<()> {
    let files = match layout {
        TreeLayout::Split(split) => render_tree(input, doc, config, split, overview_budget)?,
        TreeLayout::Skill => {
            let mut files = render_tree(input, doc, config, SplitArg::Endpoint, None)?;
            apply_skill_profile(&mut files, input, doc, config, overview_budget)?;
            files
        }
    };
    write_files(output, &files)
}

fn render_tree(
    input: &Path,
    doc: &ApiDocumentation,
    config: &DocConfig,
    split: SplitArg,
    overview_budget: Option<usize>,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    // Global guidance can be essential to interpreting an operation. Preserve
    // the full preamble in a linked page instead of loading it into the overview.
    let mut api_details = Vec::new();
    super::views::write_preamble(&mut api_details, doc, config)?;
    writeln!(api_details, "[Overview](index.md)\n")?;
    files.insert("api.md".to_string(), api_details);
    let endpoints = visible_endpoints(doc, config);
    let mut overview_entries = Vec::new();
    let mut references = BTreeSet::new();

    match split {
        SplitArg::Endpoint => {
            for endpoint in &endpoints {
                let path = format!(
                    "endpoints/{}",
                    filename(&endpoint_identity(endpoint), &endpoint_identity(endpoint))
                );
                overview_entries.push(entry(endpoint, &path));
                let mut body = Vec::new();
                writeln!(
                    body,
                    "# {}\n\n[Overview](../index.md)\n",
                    escape(&endpoint_identity(endpoint))
                )?;
                write_detail(&mut body, endpoint, doc, config, &mut references)?;
                body.extend(page_notice(input, split, config, Some(endpoint)).as_bytes());
                files.insert(path, body);
            }
        }
        SplitArg::Service | SplitArg::Tag => {
            let directory = if split == SplitArg::Tag {
                "tags"
            } else {
                "services"
            };
            // Service names are the parser's normalized tags, including its fallback.
            let names: BTreeSet<&str> = endpoints
                .iter()
                .flat_map(|ep| ep.services.iter().map(String::as_str))
                .filter(|name| service_is_visible(name, config))
                .collect();
            for name in names {
                let path = format!("{directory}/{}", filename(name, name));
                let members: Vec<_> = endpoints
                    .iter()
                    .filter(|ep| ep.services.iter().any(|service| service == name))
                    .collect();
                let mut body = Vec::new();
                writeln!(body, "# {}\n\n[Overview](../index.md)\n", escape(name))?;
                if let Some(description) = doc
                    .services
                    .iter()
                    .find(|service| service.name == name)
                    .and_then(|service| service.description.as_ref())
                {
                    writeln!(body, "{}\n", description)?;
                }
                if config.include_toc {
                    writeln!(body, "## Operations\n")?;
                    for endpoint in &members {
                        body.extend(
                            entry(endpoint, &format!("#{}", endpoint_anchor(endpoint))).as_bytes(),
                        );
                    }
                    writeln!(body)?;
                }
                for endpoint in members {
                    overview_entries.push(entry(
                        endpoint,
                        &format!("{path}#{}", endpoint_anchor(endpoint)),
                    ));
                    write_detail(&mut body, endpoint, doc, config, &mut references)?;
                }
                body.extend(page_notice(input, split, config, None).as_bytes());
                files.insert(path, body);
            }
        }
    }

    // Expand each shared definition exactly once; newly encountered references
    // are queued. The seen set terminates cycles without dropping valid links.
    let mut seen = BTreeSet::new();
    while let Some(reference) = references.pop_first() {
        if !seen.insert(reference.clone()) {
            continue;
        }
        let schema =
            resolve_schema_reference(&reference, doc).context("registered schema must resolve")?;
        let name = short_schema_reference(&reference);
        let mut body = Vec::new();
        writeln!(body, "# {}\n\n[Overview](../index.md)\n", escape(&name))?;
        let mut ctx = SchemaContext::external(doc);
        write_schema_table(&mut body, schema, &name, &mut ctx)?;
        references.extend(
            ctx.references()
                .filter(|reference| !seen.contains(*reference))
                .cloned(),
        );
        writeln!(
            body,
            "\n> Referenced schema details are in the linked schema files; they are omitted from this page.\n"
        )?;
        files.insert(format!("schemas/{}", schema_filename(&reference)), body);
    }

    let mut header = format!(
        "# {}\n\nAPI Version: {}\n\nThis compact overview omits endpoint and schema detail. Follow the operation links to the requested **{}** detail pages, then follow their schema links. Detail pages are independent of the overview budget.\n\n",
        escape(&doc.title),
        escape(&doc.version),
        detail_level_name(&config.detail_level)
    );
    header.push_str("[API details](api.md) contain the global description and usage guidance.\n\n");
    let omitted = doc.endpoints.len().saturating_sub(endpoints.len());
    if omitted > 0 {
        header.push_str(&format!("> Filters/selectors omitted {omitted} operations from this tree; they are still present in the spec. Retrieve the complete tree without filters:\n\n```sh\n{}\n```\n\n", retrieval(input, split, None)));
    }
    if config.include_auth {
        for server in &doc.servers {
            header.push_str(&format!("- Server: {}\n", escape(server)));
        }
        for (name, scheme) in &doc.security_schemes {
            header.push_str(&format!(
                "- Authentication {}: {}\n",
                escape(name),
                escape(scheme)
            ));
        }
        header.push('\n');
    }
    if endpoints.is_empty() {
        header.push_str("No operations matched the requested scope.\n\n");
    }
    if config.include_report {
        let mut report_body = Vec::new();
        crate::report::write_report(&mut report_body, &crate::report::analyze(doc, config))?;
        files.insert("report.md".to_string(), report_body);
        header.push_str("[Spec hygiene report](report.md)\n\n");
    }
    let full = format!("{header}## Operations\n\n{}", overview_entries.concat());
    let mut index = full.clone();
    if let Some(budget) = overview_budget
        && estimate_tokens(index.as_bytes()) > budget
    {
        let prefix = format!(
            "{header}> Overview entries are omitted to meet the requested {budget}-token estimate. [Complete operation map](index-all.md) lists every generated detail page.\n\n## Operations\n\n"
        );
        index = prefix;
        for entry in &overview_entries {
            if estimate_tokens(format!("{index}{entry}").as_bytes()) > budget {
                break;
            }
            index.push_str(entry);
        }
        files.insert("index-all.md".to_string(), full.into_bytes());
        if estimate_tokens(index.as_bytes()) > budget {
            eprintln!(
                "vimanam: overview navigation needs ~{} tokens, over the {budget}-token estimate; detail pages are unchanged",
                estimate_tokens(index.as_bytes())
            );
        }
    }
    files.insert("index.md".to_string(), index.into_bytes());
    Ok(files)
}

/// Skill output is a navigation profile over the endpoint tree. Details are
/// finalized first, then measured in hubs, then hubs are measured in the root.
/// Keeping estimates out of their target files avoids circular read costs.
fn apply_skill_profile(
    files: &mut BTreeMap<String, Vec<u8>>,
    input: &Path,
    doc: &ApiDocumentation,
    config: &DocConfig,
    overview_budget: Option<usize>,
) -> Result<()> {
    let endpoints = visible_endpoints(doc, config);
    for (path, body) in files.iter_mut() {
        if path.starts_with("endpoints/") {
            writeln!(
                body,
                "\n[Operation directory](index.md) · [Schema directory](../schemas/index.md)\n"
            )?;
        } else if path.starts_with("schemas/") {
            writeln!(body, "\n[Schema directory](index.md)\n")?;
        }
    }
    let cost = |path: &str| estimate_tokens(&files[path]);
    let cost_entry = |endpoint: &Endpoint, target: &str, path: &str| {
        format!(
            "{} — ~{} tokens\n",
            entry(endpoint, target).trim_end(),
            cost(path)
        )
    };
    let mut endpoint_hub = String::from(
        "# Operations\n\n[Skill entry point](../SKILL.md) · [Services](../services/index.md)\n\nEstimates use characters/4, rounded up, for the individual linked file only. Open an operation, then follow its schema links as needed.\n\n",
    );
    for endpoint in &endpoints {
        let name = filename(&endpoint_identity(endpoint), &endpoint_identity(endpoint));
        endpoint_hub.push_str(&cost_entry(endpoint, &name, &format!("endpoints/{name}")));
    }
    if endpoints.is_empty() {
        endpoint_hub.push_str("No operations matched the requested scope.\n");
    }
    let mut schema_hub = String::from(
        "# Schemas\n\n[Skill entry point](../SKILL.md) · [Operations](../endpoints/index.md)\n\nEstimates use characters/4, rounded up, for each individual file; referenced schemas are separate reads. This directory contains schemas reached by the selected operations under the requested detail options. Other schemas may still exist in the source spec.\n\n",
    );
    for (path, body) in files
        .iter()
        .filter(|(path, _)| path.starts_with("schemas/"))
    {
        let text = String::from_utf8_lossy(body);
        let name = text
            .lines()
            .next()
            .unwrap_or("# Schema")
            .trim_start_matches("# ");
        let target = path.strip_prefix("schemas/").expect("schema prefix");
        schema_hub.push_str(&format!("- [{name}]({target}) — ~{} tokens\n", cost(path)));
    }
    if !files.keys().any(|path| path.starts_with("schemas/")) {
        schema_hub.push_str(&format!(
            "Schema tables are absent under these options or no component schemas were reached. Absence here does not mean the spec has no schemas. Retrieve full schema documentation:\n\n```sh\n{}\n```\n",
            retrieval(input, SplitArg::Endpoint, None)
        ));
    }
    let names: BTreeSet<&str> = endpoints
        .iter()
        .flat_map(|ep| ep.services.iter().map(String::as_str))
        .filter(|name| service_is_visible(name, config))
        .collect();
    let mut service_pages = Vec::new();
    for name in names {
        let path = format!("services/{}", filename(name, name));
        let mut body = format!(
            "# {}\n\n[Services](index.md) · [Skill entry point](../SKILL.md)\n\nEstimates use characters/4, rounded up, for each individual linked file only.\n\n",
            escape(name)
        );
        if let Some(description) = doc
            .services
            .iter()
            .find(|service| service.name == name)
            .and_then(|service| service.description.as_deref())
        {
            body.push_str(&format!("{description}\n\n"));
        }
        let members: Vec<_> = endpoints
            .iter()
            .filter(|ep| ep.services.iter().any(|service| service == name))
            .collect();
        for endpoint in &members {
            let endpoint_path = format!(
                "endpoints/{}",
                filename(&endpoint_identity(endpoint), &endpoint_identity(endpoint))
            );
            body.push_str(&cost_entry(
                endpoint,
                &format!("../{endpoint_path}"),
                &endpoint_path,
            ));
        }
        service_pages.push((name, path, members.len(), body));
    }
    files.insert("endpoints/index.md".to_string(), endpoint_hub.into_bytes());
    files.insert("schemas/index.md".to_string(), schema_hub.into_bytes());
    let mut area_entries = Vec::new();
    let mut service_hub = String::from(
        "# Services\n\n[Skill entry point](../SKILL.md) · [All operations](../endpoints/index.md)\n\nEach hub lists method/path, operation ID, a short description and the cost of the individual endpoint read. Estimates use characters/4, rounded up.\n\n",
    );
    for (name, path, count, body) in service_pages {
        let tokens = estimate_tokens(body.as_bytes());
        area_entries.push(format!(
            "- [{}]({path}) — {count} operations; ~{tokens} tokens\n",
            escape(name)
        ));
        service_hub.push_str(&format!(
            "- [{}]({}) — {count} operations; ~{tokens} tokens\n",
            escape(name),
            path.strip_prefix("services/").expect("service prefix")
        ));
        files.insert(path, body.into_bytes());
    }
    files.insert("services/index.md".to_string(), service_hub.into_bytes());
    let name = skill_name(&doc.title);
    let description = format!(
        "Navigate {} API documentation by service, operation and schema; load only files relevant to the task.",
        doc.title
    );
    // JSON quoted strings are valid YAML scalars, including untrusted spec text.
    let mut header = format!(
        "---\nname: {}\ndescription: {}\nversion: {}\n---\n\n# {}\n\nStart with this map. Choose a service hub or the operation directory, read the relevant endpoint, then follow schema links only as needed. Every read is optional and selected by the agent.\n\nThis map omits endpoint and schema content. Detail pages retain the requested **{}** level independently of this entry point's budget. Reduced-detail pages explain omissions and give a command to retrieve fuller detail. Missing documentation here does not mean a field or operation is absent from the spec.\n\nRead costs are approximate **characters/4**, rounded up, measured from the emitted individual file. They exclude linked files, which are separate reads.\n\n",
        serde_json::to_string(&name)?,
        serde_json::to_string(&description)?,
        serde_json::to_string(&doc.version)?,
        escape(&doc.title),
        detail_level_name(&config.detail_level),
    );
    for (label, path) in [
        ("API description and usage guidance", "api.md"),
        ("Operation directory", "endpoints/index.md"),
        ("Schema directory", "schemas/index.md"),
        ("Service directory", "services/index.md"),
    ] {
        header.push_str(&format!(
            "- [{label}]({path}) — ~{} tokens\n",
            estimate_tokens(&files[path])
        ));
    }
    if let Some(report) = files.get("report.md") {
        header.push_str(&format!(
            "- [Spec hygiene report](report.md) — ~{} tokens\n",
            estimate_tokens(report)
        ));
    }
    let omitted = doc.endpoints.len().saturating_sub(endpoints.len());
    if omitted > 0 {
        header.push_str(&format!("\n> Filters/selectors omitted {omitted} operations from this tree; they remain in the spec. Retrieve all operations without filters:\n\n```sh\n{}\n```\n", retrieval(input, SplitArg::Endpoint, None)));
    }
    let full = format!("{header}\n## Services\n\n{}", area_entries.concat());
    let budget = overview_budget.unwrap_or(1600);
    let mut skill = full;
    if estimate_tokens(skill.as_bytes()) > budget {
        skill = format!(
            "{header}\n> Service entries are omitted for the requested {budget}-token estimate. [Complete map](index.md) retains every service; endpoint and schema files are unchanged.\n\n## Services\n\n"
        );
        for entry in &area_entries {
            if estimate_tokens(format!("{skill}{entry}").as_bytes()) > budget {
                break;
            }
            skill.push_str(entry);
        }
        if estimate_tokens(skill.as_bytes()) > budget {
            eprintln!(
                "vimanam: skill navigation needs ~{} tokens, over the {budget}-token estimate; detail pages are unchanged",
                estimate_tokens(skill.as_bytes())
            );
        }
    }
    let mut index = format!(
        "# {} — Complete map\n\n[Skill entry point](SKILL.md) · [All operations](endpoints/index.md) · [All rendered schemas](schemas/index.md)\n\n## Services\n\n{}",
        escape(&doc.title),
        area_entries.concat()
    );
    if endpoints.is_empty() {
        index.push_str("No operations matched the requested scope.\n");
    }
    files.insert("index.md".to_string(), index.into_bytes());
    files.insert("SKILL.md".to_string(), skill.into_bytes());
    Ok(())
}

fn write_detail(
    body: &mut Vec<u8>,
    endpoint: &Endpoint,
    doc: &ApiDocumentation,
    config: &DocConfig,
    references: &mut BTreeSet<String>,
) -> Result<()> {
    if config.detail_level == DetailLevel::Summary {
        // Summary never invokes write_endpoint (whose lowest body level is Basic).
        writeln!(
            body,
            "### {} {{#{}}}\n",
            escape(&endpoint_identity(endpoint)),
            endpoint_anchor(endpoint)
        )?;
        body.extend(entry(endpoint, &format!("#{}", endpoint_anchor(endpoint))).as_bytes());
        return Ok(());
    }
    let mut ctx = SchemaContext::external(doc);
    write_endpoint(
        body,
        endpoint,
        config,
        Some(&endpoint_anchor(endpoint)),
        &mut ctx,
    )?;
    references.extend(ctx.references().cloned());
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u8,
    files: BTreeMap<String, String>,
}

fn validate_relative(path: &str) -> Result<()> {
    let valid = path == "index.md"
        || path == "SKILL.md"
        || path == "index-all.md"
        || path == "report.md"
        || path == "api.md"
        || ["services/", "tags/", "endpoints/", "schemas/"]
            .iter()
            .any(|prefix| {
                path.starts_with(prefix) && path.ends_with(".md") && path.matches('/').count() == 1
            });
    if !valid
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.contains('\\')
    {
        bail!("Invalid generated path in manifest: {path}");
    }
    Ok(())
}

fn check_no_symlinks(path: &Path) -> Result<()> {
    // Check all ancestors, including ancestors above the output directory.
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("Refusing symlink output path: {}", ancestor.display())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("Inspecting {}", ancestor.display()));
            }
        }
    }
    Ok(())
}

fn write_files(output: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    // Absolute paths make ancestor checks work even for relative output paths.
    let output = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()?.join(output)
    };
    check_no_symlinks(&output)?;
    if output.exists() && !output.is_dir() {
        bail!("Split output must be a directory: {}", output.display());
    }
    let manifest_path = output.join(MANIFEST);
    check_no_symlinks(&manifest_path)?;
    let previous: Manifest = if manifest_path.exists() {
        serde_json::from_slice(&fs::read(&manifest_path)?)
            .context("Invalid Vimanam output manifest")?
    } else {
        Manifest {
            version: 1,
            files: BTreeMap::new(),
        }
    };
    if previous.version != 1 {
        bail!("Unsupported Vimanam output manifest version");
    }
    let paths: BTreeSet<_> = previous.files.keys().chain(files.keys()).collect();
    // No mutations until every existing target and parent has passed inspection.
    for path in paths {
        validate_relative(path)?;
        let target = output.join(path);
        check_no_symlinks(&target)?;
        if let Some(parent) = target.parent()
            && parent.exists()
            && !parent.is_dir()
        {
            bail!("Output parent is not a directory: {}", parent.display());
        }
        if target.exists() {
            let hash = sha256_hex(
                &fs::read(&target).with_context(|| format!("Reading {}", target.display()))?,
            );
            match previous.files.get(path) {
                Some(expected) if expected == &hash => {}
                Some(_) => bail!(
                    "Refusing to overwrite edited generated file: {}",
                    target.display()
                ),
                None => bail!("Refusing to overwrite unmanaged file: {}", target.display()),
            }
        }
    }
    fs::create_dir_all(&output)?;
    for (path, bytes) in files {
        let target = output.join(path);
        fs::create_dir_all(target.parent().expect("generated file has parent"))?;
        // Keep mtimes of unchanged files as well as their bytes.
        if previous
            .files
            .get(path)
            .is_some_and(|hash| hash == &sha256_hex(bytes))
            && target.exists()
        {
            continue;
        }
        fs::write(&target, bytes).with_context(|| format!("Writing {}", target.display()))?;
    }
    for path in previous
        .files
        .keys()
        .filter(|path| !files.contains_key(*path))
    {
        let target = output.join(path);
        if target.exists() {
            fs::remove_file(target)?;
        }
    }
    let manifest = Manifest {
        version: 1,
        files: files
            .iter()
            .map(|(path, bytes)| (path.clone(), sha256_hex(bytes)))
            .collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&manifest)?;
    bytes.push(b'\n');
    fs::write(manifest_path, bytes)?;
    Ok(())
}
