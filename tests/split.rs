use assert_cmd::Command;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const REFS: &str = "tests/fixtures/schema_refs_oas3.json";

fn skill_command(spec: &str, directory: &Path) -> Command {
    let mut command = Command::cargo_bin("vimanam").unwrap();
    command
        .arg(spec)
        .args([
            "--output-mode",
            "skill",
            "--detail",
            "full",
            "--include-schemas",
            "--no-report",
            "-o",
        ])
        .arg(directory);
    command
}

fn assert_read_costs(directory: &Path, files: &BTreeMap<PathBuf, String>) {
    let mut checked = 0;
    for (path, text) in files
        .iter()
        .filter(|(path, _)| path.extension().is_some_and(|ext| ext == "md"))
    {
        for line in text
            .lines()
            .filter(|line| line.starts_with("- [") && line.ends_with(" tokens"))
        {
            let target = line.split("](").nth(1).unwrap().split(')').next().unwrap();
            let expected: usize = line
                .rsplit('~')
                .next()
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let contents =
                fs::read_to_string(directory.join(path.parent().unwrap()).join(target)).unwrap();
            assert_eq!(
                expected,
                contents.chars().count().div_ceil(4),
                "cost in {} for {target}",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 5);
}

#[test]
fn skill_tree_supports_selected_reads_frontmatter_and_exact_individual_costs() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().canonicalize().unwrap().join("skill");
    skill_command(REFS, &directory).assert().success();
    let files = tree(&directory);
    assert_links(&directory, &files);
    assert_read_costs(&directory, &files);
    let root = &files[Path::new("SKILL.md")];
    assert!(root.starts_with("---\nname: "));
    assert!(root.contains("\ndescription: "));
    assert!(root.contains("\nversion: "));
    assert!(root.contains("characters/4"));
    assert!(root.contains("individual file"));
    assert!(!root.contains("Category.identifier"));
    let operations = &files[Path::new("endpoints/index.md")];
    let chosen = operations
        .lines()
        .find(|line| line.contains("Pets\\_CreatePet"))
        .unwrap();
    assert!(chosen.contains("POST /pets"));
    let endpoint_name = chosen
        .split("](")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap();
    let endpoint = &files[&PathBuf::from("endpoints").join(endpoint_name)];
    assert!(endpoint.contains("../schemas/pet-"));
    assert!(!endpoint.contains("Node.next"));
    assert!(!endpoint.contains("Category.identifier"));
    let pet_link = endpoint
        .split("](../schemas/")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap();
    let pet = &files[&PathBuf::from("schemas").join(pet_link)];
    assert!(pet.contains("CreatePetRequest.category"));
    let category_link = pet
        .split("](../schemas/")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap();
    assert!(files[&PathBuf::from("schemas").join(category_link)].contains("Category.id"));
    skill_command(REFS, &directory).assert().success();
    assert_eq!(tree(&directory), files);
}

#[test]
fn skill_overview_budget_preserves_every_other_artifact() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let full = root.join("full");
    let compact = root.join("compact");
    skill_command(REFS, &full).assert().success();
    skill_command(REFS, &compact)
        .args(["--overview-max-tokens", "1"])
        .assert()
        .success();
    let compact_files = tree(&compact);
    assert_links(&compact, &compact_files);
    assert_read_costs(&compact, &compact_files);
    assert!(compact_files[Path::new("SKILL.md")].contains("[Complete map](index.md)"));
    for (path, bytes) in tree(&full) {
        if path != Path::new("SKILL.md") && path != Path::new(".vimanam-manifest.json") {
            assert_eq!(compact_files[&path], bytes);
        }
    }
}

#[test]
fn skill_names_are_bounded_ascii_hyphen_case_and_metadata_round_trips() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let titles = [
        "Endor_Labs API".to_string(),
        "文档".to_string(),
        "!".to_string(),
        format!("API {}", "A".repeat(80)),
        format!("{} XYZ", "A".repeat(59)),
        "--Hostile_\"API\"\nversion: injected--".to_string(),
    ];
    let version = "1.0:\n\"release\"";
    let mut spec: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(REFS).unwrap()).unwrap();
    for (index, title) in titles.iter().enumerate() {
        spec["info"]["title"] = serde_json::json!(title);
        spec["info"]["version"] = serde_json::json!(version);
        let input = root.join(format!("spec{index}.json"));
        let directory = root.join(format!("skill{index}"));
        fs::write(&input, serde_json::to_vec(&spec).unwrap()).unwrap();
        skill_command(input.to_str().unwrap(), &directory)
            .assert()
            .success();
        let contents = fs::read_to_string(directory.join("SKILL.md")).unwrap();
        let frontmatter = contents.split("---\n").nth(1).unwrap();
        let metadata: serde_norway::Value = serde_norway::from_str(frontmatter).unwrap();
        let name = metadata["name"].as_str().unwrap();
        assert!(!name.is_empty() && name.len() <= 64, "{title}: {name}");
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
            "{title}: {name}"
        );
        assert!(
            !name.starts_with('-') && !name.ends_with('-') && !name.contains("--"),
            "{title}: {name}"
        );
        assert_eq!(
            metadata["description"].as_str().unwrap(),
            format!(
                "Navigate {title} API documentation by service, operation and schema; load only files relevant to the task."
            )
        );
        assert_eq!(metadata["version"].as_str().unwrap(), version);
        skill_command(input.to_str().unwrap(), &directory)
            .assert()
            .success();
        assert_eq!(
            fs::read_to_string(directory.join("SKILL.md")).unwrap(),
            contents
        );
    }
}

#[test]
fn skill_filters_multitags_and_reduced_details_preserve_navigation_and_retrieval() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let directory = root.join("skill");
    skill_command("tests/fixtures/multi_tag_oas3.json", &directory)
        .assert()
        .success();
    let full = tree(&directory);
    assert_links(&directory, &full);
    assert_eq!(
        full.keys()
            .filter(|path| path.starts_with("endpoints") && path.file_name().unwrap() != "index.md")
            .count(),
        1
    );
    assert_eq!(
        full.keys()
            .filter(|path| path.starts_with("services") && path.file_name().unwrap() != "index.md")
            .count(),
        2
    );
    let filtered = root.join("filtered");
    Command::cargo_bin("vimanam")
        .unwrap()
        .arg(REFS)
        .args([
            "--output-mode",
            "skill",
            "--operation-id",
            "Pets_CreatePet",
            "--detail",
            "summary",
            "-o",
        ])
        .arg(&filtered)
        .assert()
        .success();
    let files = tree(&filtered);
    assert_links(&filtered, &files);
    assert!(files[Path::new("SKILL.md")].contains("omitted 1 operations"));
    assert!(files[Path::new("schemas/index.md")].contains("Retrieve full schema documentation"));
    let endpoint = files
        .iter()
        .find(|(path, _)| path.starts_with("endpoints") && path.file_name().unwrap() != "index.md")
        .unwrap()
        .1;
    assert!(endpoint.contains("Retrieve fuller detail"));
    assert!(endpoint.contains("--operation 'POST /pets'"));
    assert!(!endpoint.contains("#### Parameters"));
    skill_command("tests/fixtures/petstore_oas2.json", &root.join("oas2"))
        .assert()
        .success();
    assert_links(&root.join("oas2"), &tree(&root.join("oas2")));
}

#[test]
fn skill_requires_directory_and_rejects_conflicting_output_options() {
    for args in [
        vec!["--output-mode", "skill"],
        vec!["--output-mode", "skill", "-o", "x", "--split", "endpoint"],
        vec!["--output-mode", "skill", "-o", "x", "--max-tokens", "8"],
        vec!["--output-mode", "skill", "-o", "x", "--inline-schemas"],
        vec!["--output-mode", "skill", "-o", "x", "--stats"],
    ] {
        Command::cargo_bin("vimanam")
            .unwrap()
            .arg(REFS)
            .args(args)
            .assert()
            .code(2);
    }
}

fn command(spec: &str, directory: &Path, mode: &str) -> Command {
    let mut command = Command::cargo_bin("vimanam").unwrap();
    command
        .arg(spec)
        .args([
            "--split",
            mode,
            "--detail",
            "full",
            "--include-schemas",
            "--no-report",
            "-o",
        ])
        .arg(directory);
    command
}

fn tree(directory: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read_to_string(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(directory, directory, &mut files);
    files
}

fn assert_links(directory: &Path, files: &BTreeMap<PathBuf, String>) {
    for (path, text) in files
        .iter()
        .filter(|(path, _)| path.extension().is_some_and(|extension| extension == "md"))
    {
        for rest in text.split("](").skip(1) {
            let target = rest.split(')').next().unwrap();
            if target.contains("://") {
                continue;
            }
            let (file, anchor) = target
                .split_once('#')
                .map(|(file, anchor)| (file, Some(anchor)))
                .unwrap_or((target, None));
            let target = if file.is_empty() {
                directory.join(path)
            } else {
                directory.join(path.parent().unwrap()).join(file)
            };
            assert!(
                target.is_file(),
                "{} -> {}",
                path.display(),
                target.display()
            );
            if let Some(anchor) = anchor {
                assert!(
                    fs::read_to_string(&target)
                        .unwrap()
                        .contains(&format!("{{#{anchor}}}")),
                    "missing anchor {anchor} in {}",
                    target.display()
                );
            }
        }
    }
}

#[test]
fn split_modes_preserve_schema_details_and_valid_cycle_links() {
    for mode in ["service", "tag", "endpoint"] {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().canonicalize().unwrap().join("docs");
        command(REFS, &directory, mode).assert().success();
        let files = tree(&directory);
        assert_links(&directory, &files);
        assert_eq!(
            files
                .keys()
                .filter(|path| path.starts_with("schemas"))
                .count(),
            4
        );
        let node = files
            .iter()
            .find(|(path, _)| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("node-")
            })
            .unwrap()
            .1;
        assert!(node.contains("Node.next"));
        assert!(node.contains("../schemas/node-"));
        for (path, text) in &files {
            if path.starts_with("endpoints")
                || path.starts_with("services")
                || path.starts_with("tags")
            {
                assert!(!text.contains("Schema Definitions"));
                assert!(!text.contains("Category.identifier"));
            }
        }
        command(REFS, &directory, mode).assert().success();
        assert_eq!(tree(&directory), files);
    }
}

#[test]
fn overview_budget_keeps_complete_navigation_and_identical_detail() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let full = root.join("full");
    let compact = root.join("compact");
    command(REFS, &full, "endpoint").assert().success();
    command(REFS, &compact, "endpoint")
        .args(["--overview-max-tokens", "1"])
        .assert()
        .success();
    let compact_files = tree(&compact);
    assert_links(&compact, &compact_files);
    assert!(compact_files[Path::new("index.md")].contains("Complete operation map"));
    assert!(compact_files[Path::new("index-all.md")].contains("Nodes\\_GetNode"));
    for (path, bytes) in tree(&full) {
        if path.starts_with("endpoints") || path.starts_with("schemas") {
            assert_eq!(compact_files[&path], bytes);
        }
    }
}

#[test]
fn filters_preserve_paths_links_and_visible_retrieval_guidance() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let full = root.join("full");
    let filtered = root.join("filtered");
    command(REFS, &full, "service").assert().success();
    command(REFS, &filtered, "service")
        .args(["--operation-id", "Pets_CreatePet"])
        .assert()
        .success();
    let files = tree(&filtered);
    assert_links(&filtered, &files);
    let overview = &files[Path::new("index.md")];
    assert!(overview.contains("omitted 1 operations"));
    assert!(overview.contains("vimanam 'tests/fixtures/schema_refs_oas3.json' --split service"));
    assert!(!files.keys().any(|path| {
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("node-")
    }));
    let full_files = tree(&full);
    for path in files.keys().filter(|path| path.starts_with("schemas")) {
        assert!(full_files.contains_key(path));
    }
}

#[test]
fn split_rejects_conflicting_budget_inline_and_missing_directory() {
    for args in [
        vec!["--split", "endpoint"],
        vec!["--split", "endpoint", "-o", "x", "--max-tokens", "8"],
        vec!["--split", "endpoint", "-o", "x", "--inline-schemas"],
        vec!["--overview-max-tokens", "8"],
    ] {
        Command::cargo_bin("vimanam")
            .unwrap()
            .arg(REFS)
            .args(args)
            .assert()
            .code(2);
    }
}

#[test]
fn output_preflights_collisions_protects_edits_and_removes_only_owned_stale_pages() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let directory = root.join("docs");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("index.md"), "user content").unwrap();
    command(REFS, &directory, "endpoint").assert().failure();
    assert_eq!(tree(&directory).len(), 1);
    fs::remove_file(directory.join("index.md")).unwrap();
    fs::write(directory.join("notes.md"), "keep me").unwrap();
    command(REFS, &directory, "endpoint").assert().success();
    let before = tree(&directory);
    let edited = before
        .keys()
        .find(|path| path.starts_with("schemas"))
        .unwrap();
    fs::write(directory.join(edited), "local edit").unwrap();
    command(REFS, &directory, "endpoint")
        .args(["--operation-id", "Nodes_GetNode"])
        .assert()
        .failure();
    let mut expected = before.clone();
    expected.insert(edited.clone(), "local edit".to_string());
    assert_eq!(tree(&directory), expected);
    fs::write(directory.join(edited), &before[edited]).unwrap();
    command(REFS, &directory, "endpoint")
        .args(["--operation-id", "Nodes_GetNode"])
        .assert()
        .success();
    let after = tree(&directory);
    assert_links(&directory, &after);
    assert_eq!(after[Path::new("notes.md")], "keep me");
    assert_eq!(
        after
            .keys()
            .filter(|path| path.starts_with("schemas"))
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn symlink_output_and_managed_parents_are_rejected() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let actual = root.join("actual");
    fs::create_dir(&actual).unwrap();
    let link = root.join("link");
    symlink(&actual, &link).unwrap();
    command(REFS, &link, "endpoint").assert().failure();
    assert!(tree(&actual).is_empty());
    let directory = root.join("docs");
    fs::create_dir(&directory).unwrap();
    symlink(&actual, directory.join("schemas")).unwrap();
    command(REFS, &directory, "endpoint").assert().failure();
    assert!(!directory.join("index.md").exists());
    assert!(tree(&actual).is_empty());
}

#[test]
fn operation_edit_changes_only_affected_endpoint_and_navigation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let spec = root.join("spec.json");
    let directory = root.join("docs");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(REFS).unwrap()).unwrap();
    fs::write(&spec, serde_json::to_vec(&value).unwrap()).unwrap();
    command(spec.to_str().unwrap(), &directory, "endpoint")
        .assert()
        .success();
    let before = tree(&directory);
    value["paths"]["/pets"]["post"]["summary"] = serde_json::json!("Updated operation summary");
    fs::write(&spec, serde_json::to_vec(&value).unwrap()).unwrap();
    command(spec.to_str().unwrap(), &directory, "endpoint")
        .assert()
        .success();
    let after = tree(&directory);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    let changed: Vec<_> = before
        .keys()
        .filter(|path| before[*path] != after[*path])
        .collect();
    assert_eq!(changed.len(), 3); // endpoint, index, ownership manifest
    assert_eq!(
        changed
            .iter()
            .filter(|path| path.starts_with("endpoints"))
            .count(),
        1
    );
}

#[test]
fn swagger_two_and_summary_split_are_supported() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let directory = root.join("docs");
    command("tests/fixtures/petstore_oas2.json", &directory, "endpoint")
        .assert()
        .success();
    assert_links(&directory, &tree(&directory));
    Command::cargo_bin("vimanam")
        .unwrap()
        .arg(REFS)
        .args([
            "--split",
            "endpoint",
            "--detail",
            "summary",
            "--no-report",
            "-o",
        ])
        .arg(&directory)
        .assert()
        .success();
    let files = tree(&directory);
    assert_links(&directory, &files);
    assert!(!files.keys().any(|path| path.starts_with("schemas")));
    for (_, text) in files
        .iter()
        .filter(|(path, _)| path.starts_with("endpoints"))
    {
        assert!(!text.contains("#### Parameters"));
        assert!(text.contains("Retrieve fuller detail"));
    }
}

#[test]
fn colliding_slugs_and_pointer_escaped_schema_names_keep_distinct_stable_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let spec = root.join("spec.json");
    let directory = root.join("docs");
    let value = serde_json::json!({
        "openapi": "3.0.0", "info": {"title": "Names [API]", "version": "1"},
        "paths": {
            "/a/b": {"get": {"tags": ["A/B"], "operationId": "A[one]", "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/A~1B"}}}}}}},
            "/a-b": {"get": {"tags": ["A-B"], "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/A-B"}}}}}}}
        },
        "components": {"schemas": {
            "A/B": {"type": "object", "properties": {"field|`name": {"type": "string"}}},
            "A-B": {"type": "object", "properties": {"other": {"type": "integer"}}}
        }}
    });
    fs::write(&spec, serde_json::to_vec(&value).unwrap()).unwrap();
    command(spec.to_str().unwrap(), &directory, "tag")
        .assert()
        .success();
    let before = tree(&directory);
    assert_links(&directory, &before);
    assert_eq!(
        before
            .keys()
            .filter(|path| path.starts_with("tags"))
            .count(),
        2
    );
    assert_eq!(
        before
            .keys()
            .filter(|path| path.starts_with("schemas"))
            .count(),
        2
    );
    assert!(before[Path::new("index.md")].contains("A\\[one\\]"));
    assert!(
        before
            .values()
            .any(|text| text.contains("`` A/B.field\\|`name ``"))
    );
    command(spec.to_str().unwrap(), &directory, "tag")
        .args(["--operation", "GET /a/b"])
        .assert()
        .success();
    let filtered = tree(&directory);
    assert_links(&directory, &filtered);
    for path in filtered
        .keys()
        .filter(|path| path.starts_with("schemas") || path.starts_with("tags"))
    {
        assert!(before.contains_key(path));
    }
}

#[test]
fn global_api_guidance_remains_available_in_all_modes_and_tiny_overviews() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let spec = root.join("guidance.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(REFS).unwrap()).unwrap();
    let guidance = format!(
        "GLOBAL_GUIDANCE_SENTINEL\n\n{}",
        "Important global API usage conventions. ".repeat(1000)
    );
    value["info"]["description"] = serde_json::json!(guidance);
    fs::write(&spec, serde_json::to_vec(&value).unwrap()).unwrap();
    for mode in ["service", "tag", "endpoint"] {
        let directory = root.join(mode);
        command(spec.to_str().unwrap(), &directory, mode)
            .assert()
            .success();
        let complete = tree(&directory);
        assert!(complete[Path::new("api.md")].contains(&guidance));
        assert!(complete[Path::new("index.md")].contains("[API details](api.md)"));
        assert!(!complete[Path::new("index.md")].contains("GLOBAL_GUIDANCE_SENTINEL"));
        command(spec.to_str().unwrap(), &directory, mode)
            .args(["--overview-max-tokens", "1"])
            .assert()
            .success();
        let budgeted = tree(&directory);
        assert_links(&directory, &budgeted);
        assert!(budgeted[Path::new("index.md")].contains("[API details](api.md)"));
        assert_eq!(budgeted[Path::new("api.md")], complete[Path::new("api.md")]);
        let manifest: serde_json::Value =
            serde_json::from_str(&budgeted[Path::new(".vimanam-manifest.json")]).unwrap();
        assert!(manifest["files"]["api.md"].is_string());
        // Regeneration reads and validates the newly owned root-level path.
        command(spec.to_str().unwrap(), &directory, mode)
            .args(["--overview-max-tokens", "1"])
            .assert()
            .success();
        assert_eq!(tree(&directory), budgeted);
    }
}

#[test]
fn depth_limits_apply_to_split_and_skill_reference_graphs_with_valid_links() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    for (mode, value) in [
        ("--split", "endpoint"),
        ("--split", "service"),
        ("--split", "tag"),
        ("--output-mode", "skill"),
    ] {
        for depth in [0, 3, 5, 8] {
            let directory = root.join(format!("{}-{value}-{depth}", mode.trim_start_matches('-')));
            Command::cargo_bin("vimanam")
                .unwrap()
                .args([
                    "tests/fixtures/schema_selection_oas3.json",
                    mode,
                    value,
                    "--detail",
                    "full",
                    "--include-schemas",
                    "--schema-depth",
                    &depth.to_string(),
                    "-o",
                ])
                .arg(&directory)
                .assert()
                .success();
            let files = tree(&directory);
            assert_links(&directory, &files);
            let schemas: Vec<_> = files
                .iter()
                .filter(|(path, _)| {
                    path.starts_with("schemas") && path.file_name().unwrap() != "index.md"
                })
                .collect();
            if depth == 0 {
                assert!(schemas.is_empty());
            } else if depth == 3 {
                assert!(
                    schemas
                        .iter()
                        .any(|(_, text)| text.starts_with("# Shared\n"))
                );
                assert!(!schemas.iter().any(|(_, text)| text.starts_with("# Tag\n")));
                assert!(!schemas.iter().any(|(_, text)| text.starts_with("# Tail\n")));
                let root = schemas
                    .iter()
                    .find(|(_, text)| text.starts_with("# Root\n"))
                    .expect("Root schema page");
                assert!(
                    root.1.contains("[Shared](../schemas/shared-"),
                    "cutoff hop should link to emitted Shared: {}",
                    root.1
                );
                assert!(
                    root.1.contains("| ` Root.selected[] ` | ref Tag |"),
                    "missing Tag must stay an unlinked ref: {}",
                    root.1
                );
            } else if depth == 5 {
                // Shared is first discovered via Root.deep.hop (depth4), then
                // Root.shallow (depth3). The shallower path must expose tail.
                let shared = schemas
                    .iter()
                    .find(|(_, text)| text.starts_with("# Shared\n"))
                    .unwrap();
                assert!(shared.1.contains("../schemas/tail-"));
                assert!(schemas.iter().any(|(_, text)| text.starts_with("# Tail\n")));
            }
            let detail = files
                .iter()
                .filter(|(path, _)| path.extension().is_some_and(|ext| ext == "md"))
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if depth < 8 {
                assert!(detail.contains("Omitted nested expansion"));
            }
            Command::cargo_bin("vimanam")
                .unwrap()
                .args([
                    "tests/fixtures/schema_selection_oas3.json",
                    mode,
                    value,
                    "--detail",
                    "full",
                    "--include-schemas",
                    "--schema-depth",
                    &depth.to_string(),
                    "-o",
                ])
                .arg(&directory)
                .assert()
                .success();
            assert_eq!(tree(&directory), files);
        }
    }
}
