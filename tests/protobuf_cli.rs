#![cfg(feature = "cli")]
use assert_cmd::Command as AssertCommand;
use serde_json::Value;
use std::{fs, path::Path, process::Command};
fn fixture(n: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/protobuf")
        .join(n)
}
fn binary() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("vimanam"))
}
fn file(temp: &tempfile::TempDir, n: &str, text: &str) -> std::path::PathBuf {
    let p = temp.path().join(n);
    fs::write(&p, text).unwrap();
    p
}
#[test]
fn descriptor_only_json_works_without_protoc() {
    let output = binary()
        .env("PATH", "")
        .args(["diff"])
        .arg(fixture("health-v1.0.0.pb"))
        .arg(fixture("health-v1.20.0.pb"))
        .args(["--format", "json", "--report"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["summary"]["operations_added"], 1);
}

#[test]
fn nested_source_paths_match_descriptor_file_option_facts() {
    let t = tempfile::tempdir().unwrap();
    for version in ["old", "new"] {
        let dir = t.path().join(version).join("nested");
        fs::create_dir_all(&dir).unwrap();
        let value = if version == "old" { "before" } else { "after" };
        fs::write(
            dir.join("main.proto"),
            format!("syntax=\"proto3\"; option java_package=\"{value}\"; import \"{version}/nested/a.proto\"; import \"{version}/nested/b.proto\"; message Main {{}}"),
        )
        .unwrap();
        fs::write(
            dir.join("a.proto"),
            format!("syntax=\"proto3\"; package a; option java_package=\"{value}\"; message A1 {{}} message A2 {{}}"),
        )
        .unwrap();
        fs::write(
            dir.join("b.proto"),
            format!("syntax=\"proto3\"; package b; option java_package=\"{value}\"; message B1 {{}} message B2 {{}}"),
        )
        .unwrap();
    }
    let root = t.path();
    for version in ["old", "new"] {
        let descriptor = root.join(format!("{version}.pb"));
        let input = format!("{version}/nested/main.proto");
        let output = Command::new("protoc")
            .current_dir(root)
            .args(["-I", ".", "--include_imports", "--include_source_info"])
            .arg(format!("--descriptor_set_out={}", descriptor.display()))
            .arg(input)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let source_json: Value = serde_json::from_slice(
        &binary()
            .arg("diff")
            .arg(root.join("old/nested/main.proto"))
            .arg(root.join("new/nested/main.proto"))
            .args(["-I"])
            .arg(root)
            .args(["--format", "json", "--report"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    let descriptor_json: Value = serde_json::from_slice(
        &binary()
            .arg("diff")
            .arg(root.join("old.pb"))
            .arg(root.join("new.pb"))
            .args(["--format", "json", "--report"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(source_json["changes"], descriptor_json["changes"]);
    assert_eq!(
        source_json["old"]["contract"],
        descriptor_json["old"]["contract"]
    );
    assert_eq!(
        source_json["new"]["contract"],
        descriptor_json["new"]["contract"]
    );
    assert_eq!(source_json["changes"].as_array().unwrap().len(), 3);
}
#[test]
fn v1_rejection_precedes_output_creation() {
    let t = tempfile::tempdir().unwrap();
    let output = t.path().join("out.json");
    binary()
        .args(["diff"])
        .arg(fixture("health-v1.0.0.pb"))
        .arg(fixture("health-v1.20.0.pb"))
        .args(["--contract", "1", "-o"])
        .arg(&output)
        .assert()
        .failure()
        .stderr(predicates::str::contains("OpenAPI-only"));
    assert!(!output.exists());
}
#[test]
fn source_breaking_scope_and_complete_report() {
    let t = tempfile::tempdir().unwrap();
    let a = file(
        &t,
        "old.proto",
        "syntax=\"proto3\";message M {string old_name=1;}",
    );
    let b = file(
        &t,
        "new.proto",
        "syntax=\"proto3\";message M {string new_name=1;}",
    );
    binary()
        .arg("diff")
        .arg(&a)
        .arg(&b)
        .args(["--fail-on-breaking", "--breaking", "wire"])
        .assert()
        .success();
    binary()
        .arg("diff")
        .arg(&a)
        .arg(&b)
        .args(["--fail-on-breaking", "--breaking", "source", "--report"])
        .assert()
        .code(3)
        .stdout(predicates::str::contains("## Report"));
}

#[test]
fn required_reserved_removal_fails_wire_gate() {
    let t = tempfile::tempdir().unwrap();
    let a = file(
        &t,
        "old.proto",
        "syntax=\"proto2\"; message M { required int32 id=1; }",
    );
    let b = file(
        &t,
        "new.proto",
        "syntax=\"proto2\"; message M { reserved 1; }",
    );
    binary()
        .arg("diff")
        .arg(&a)
        .arg(&b)
        .args(["--fail-on-breaking", "--breaking", "wire"])
        .assert()
        .code(3);
}

#[test]
fn required_field_in_new_message_does_not_fail_any_breaking_scope() {
    let t = tempfile::tempdir().unwrap();
    let a = file(&t, "old.proto", "syntax=\"proto2\"; message Existing {}");
    let b = file(
        &t,
        "new.proto",
        "syntax=\"proto2\"; message Existing {} message Added { required int32 id=1; }",
    );
    for scope in ["wire", "source", "all"] {
        binary()
            .arg("diff")
            .arg(&a)
            .arg(&b)
            .args(["--fail-on-breaking", "--breaking", scope])
            .assert()
            .success();
    }

    let c = file(
        &t,
        "surviving.proto",
        "syntax=\"proto2\"; message Existing { required int32 id=1; }",
    );
    for scope in ["wire", "source", "all"] {
        binary()
            .arg("diff")
            .arg(&a)
            .arg(&c)
            .args(["--fail-on-breaking", "--breaking", scope])
            .assert()
            .code(3);
    }
}
#[test]
fn protobuf_native_markdown_budget_and_skill() {
    let t = tempfile::tempdir().unwrap();
    let out = binary()
        .arg(fixture("health-v1.20.0.proto"))
        .args([
            "--operation",
            "RPC grpc.health.v1.Health.Watch",
            "--detail",
            "full",
            "--include-schemas",
            "--no-report",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("server streaming"));
    assert!(text.contains("Field #1"));
    assert!(text.contains("#### Result"));
    assert!(!text.contains("| 200 |"));
    assert!(!text.contains("Health.Check"));
    binary()
        .arg(fixture("health-v1.20.0.pb"))
        .args([
            "--operation-id",
            "grpc.health.v1.Health.Watch",
            "--max-tokens",
            "100",
            "--no-report",
        ])
        .assert()
        .success();
    let dir = t.path().canonicalize().unwrap().join("skill");
    binary()
        .arg(fixture("health-v1.20.0.pb"))
        .args([
            "--output-mode",
            "skill",
            "--detail",
            "full",
            "--include-schemas",
            "-o",
        ])
        .arg(&dir)
        .assert()
        .success();
    assert!(dir.join("SKILL.md").exists());
    assert!(dir.join("schemas").is_dir());
    let root = fs::read_to_string(dir.join("SKILL.md")).unwrap();
    assert!(root.contains("Protocol Buffers"));
}

#[test]
fn schema_depth_retrieval_repeats_protobuf_import_roots() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(
        root.join("api.proto"),
        "syntax=\"proto3\"; package demo; import \"pkg/types.proto\"; service S { rpc Get(pkg.Request) returns (pkg.Response); }",
    )
    .unwrap();
    fs::write(
        root.join("pkg/types.proto"),
        "syntax=\"proto3\"; package pkg; message Request { string id=1; } message Response { string result=1; }",
    )
    .unwrap();
    let output = binary()
        .arg(root.join("api.proto"))
        .args(["-I"])
        .arg(&root)
        .args([
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            "0",
            "--no-report",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let markdown = String::from_utf8(output).unwrap();
    assert!(markdown.contains(&format!("-I '{}'", root.display())));
    assert!(markdown.contains("--schema 'pkg.Request'"));
    binary()
        .arg(root.join("api.proto"))
        .args(["-I"])
        .arg(&root)
        .args(["--schema", "pkg.Request", "--no-report"])
        .assert()
        .success();
}
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let o = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}
fn historical_fixture() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    git(t.path(), &["init", "-q"]);
    git(t.path(), &["config", "user.name", "Public test"]);
    git(t.path(), &["config", "user.email", "test@example.invalid"]);
    fs::create_dir(t.path().join("deps")).unwrap();
    file(
        &t,
        "api.proto",
        "syntax=\"proto3\"; import \"dep.proto\"; service S {rpc R(M) returns(M);}",
    );
    fs::write(
        t.path().join("deps/dep.proto"),
        "syntax=\"proto3\"; message M {int32 id=1;}",
    )
    .unwrap();
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-qm", "old"]);
    git(t.path(), &["tag", "old"]);
    fs::write(
        t.path().join("deps/dep.proto"),
        "syntax=\"proto3\"; message M {string id=1;}",
    )
    .unwrap();
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-qm", "new"]);
    git(t.path(), &["tag", "new"]);
    fs::write(t.path().join("deps/dep.proto"), "broken worktree bytes").unwrap();
    t
}
#[test]
fn git_imports_use_correct_refs_not_worktree() {
    let t = historical_fixture();
    binary()
        .current_dir(t.path())
        .args([
            "diff",
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--spec",
            "api.proto",
            "-I",
            "deps",
            "--format",
            "json",
            "--fail-on-breaking",
            "--breaking",
            "wire",
        ])
        .assert()
        .code(3)
        .stdout(predicates::str::contains("FIELD_SAME_TYPE"));
}
#[test]
fn escaping_git_import_root_rejected_before_output() {
    let t = historical_fixture();
    let out = t.path().join("out.md");
    binary()
        .current_dir(t.path())
        .args([
            "diff",
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--spec",
            "api.proto",
            "-I",
            "../outside",
            "-o",
        ])
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("inside the committed repository"));
    assert!(!out.exists());
}
#[cfg(unix)]
#[test]
fn symlink_historical_protos_are_rejected() {
    let t = historical_fixture();
    std::os::unix::fs::symlink("api.proto", t.path().join("link.proto")).unwrap();
    git(t.path(), &["add", "link.proto"]);
    git(t.path(), &["commit", "-qm", "symlink"]);
    binary()
        .current_dir(t.path())
        .args([
            "diff",
            "--from-ref",
            "old",
            "--to-ref",
            "HEAD",
            "--spec",
            "api.proto",
            "-I",
            "deps",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("regular committed files"));
}
#[test]
fn descriptor_git_inputs_need_no_protoc() {
    let t = tempfile::tempdir().unwrap();
    git(t.path(), &["init", "-q"]);
    git(t.path(), &["config", "user.name", "Public test"]);
    git(t.path(), &["config", "user.email", "test@example.invalid"]);
    fs::copy(fixture("health-v1.0.0.pb"), t.path().join("api.pb")).unwrap();
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-qm", "old"]);
    git(t.path(), &["tag", "old"]);
    fs::copy(fixture("health-v1.20.0.pb"), t.path().join("api.pb")).unwrap();
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-qm", "new"]);
    binary()
        .current_dir(t.path())
        .args([
            "diff",
            "--from-ref",
            "old",
            "--to-ref",
            "HEAD",
            "--spec",
            "api.pb",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("operation_added"));
}
#[test]
fn unsupported_descriptor_syntax_leaves_output_untouched() {
    use prost::Message;
    let t = tempfile::tempdir().unwrap();
    let bad = t.path().join("bad.pb");
    let set = prost_types::FileDescriptorSet {
        file: vec![prost_types::FileDescriptorProto {
            name: Some("api.proto".into()),
            syntax: Some("editions".into()),
            ..Default::default()
        }],
    };
    fs::write(&bad, set.encode_to_vec()).unwrap();
    let out = file(&t, "existing.md", "preserve me");
    binary()
        .arg("diff")
        .arg(&bad)
        .arg(fixture("health-v1.0.0.pb"))
        .arg("-o")
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("Editions are not yet supported"));
    assert_eq!(fs::read_to_string(&out).unwrap(), "preserve me");
}
#[test]
fn unsupported_editions_source_leaves_output_untouched() {
    let t = tempfile::tempdir().unwrap();
    let bad = file(&t, "api.proto", "edition=\"2023\";message M {string id=1;}");
    let out = t.path().join("out.md");
    binary()
        .arg(&bad)
        .arg("-o")
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("Editions are not yet supported"));
    assert!(!out.exists());
}
