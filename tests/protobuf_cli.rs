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
    assert!(text.contains("Performs a watch for the serving status"));
    assert!(text.contains("Used only by the Watch method."));
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

    // Imported Editions file with leading comments is also rejected cleanly.
    let imported = file(
        &t,
        "dep.proto",
        "// leading comment\n/* block */\nedition = \"2023\";\nmessage Dep { string id = 1; }\n",
    );
    let _ = imported;
    let main = file(
        &t,
        "main.proto",
        "syntax = \"proto3\"; import \"dep.proto\"; message Main { Dep d = 1; }",
    );
    binary()
        .arg(&main)
        .arg("-o")
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("Editions are not yet supported"));
    assert!(!out.exists());
}

#[test]
fn proto_source_compiles_without_protoc_on_path() {
    let t = tempfile::tempdir().unwrap();
    let a = file(
        &t,
        "old.proto",
        r#"syntax = "proto3";
package demo.v1;
import "google/protobuf/timestamp.proto";
// Sample message.
message Event {
  string id = 1;
  google.protobuf.Timestamp created_at = 2;
}
service Events {
  // Fetch an event.
  rpc Get(Event) returns (Event);
}
"#,
    );
    let b = file(
        &t,
        "new.proto",
        r#"syntax = "proto3";
package demo.v1;
import "google/protobuf/timestamp.proto";
message Event {
  string id = 1;
  google.protobuf.Timestamp created_at = 2;
  string label = 3;
}
service Events {
  rpc Get(Event) returns (Event);
}
"#,
    );
    // Single-file Markdown rendering with empty PATH (no protoc binary available).
    let rendered = binary()
        .env("PATH", "")
        .arg(&a)
        .args(["--detail", "full", "--include-schemas", "--no-report"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let markdown = String::from_utf8(rendered).unwrap();
    assert!(markdown.contains("Fetch an event."));
    assert!(markdown.contains("Sample message."));

    // Source-to-source diff with empty PATH (no protoc binary available).
    let diff_out = binary()
        .env("PATH", "")
        .arg("diff")
        .arg(&a)
        .arg(&b)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&diff_out).unwrap();
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["summary"]["breaking"], 0);
    assert!(json["summary"]["non_breaking"].as_u64().unwrap() >= 1);
}

#[test]
fn shadowed_proto_include_roots_fail_in_file_and_git_diff() {
    let t = tempfile::tempdir().unwrap();
    let v1 = t.path().join("v1");
    let v2 = t.path().join("v2");
    fs::create_dir_all(&v1).unwrap();
    fs::create_dir_all(&v2).unwrap();
    fs::write(
        v1.join("api.proto"),
        "syntax = \"proto3\";\npackage demo;\nmessage M { string old_name = 1; }\n",
    )
    .unwrap();
    fs::write(
        v2.join("api.proto"),
        "syntax = \"proto3\";\npackage demo;\nmessage M { string new_name = 1; }\n",
    )
    .unwrap();

    // File-pair diff rejects shadowed entry ahead of earlier -I root and leaves -o untouched.
    let out = file(&t, "file_out.md", "keep existing");
    binary()
        .arg("diff")
        .arg(v2.join("api.proto"))
        .arg(v2.join("api.proto"))
        .args(["-I"])
        .arg(&v1)
        .args(["-I"])
        .arg(&v2)
        .arg("-o")
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("shadowed"));
    assert_eq!(fs::read_to_string(&out).unwrap(), "keep existing");

    // Reversing -I so v2 precedes v1 is not shadowed and succeeds.
    binary()
        .arg("diff")
        .arg(v2.join("api.proto"))
        .arg(v2.join("api.proto"))
        .args(["-I"])
        .arg(&v2)
        .args(["-I"])
        .arg(&v1)
        .assert()
        .success();

    // Git-ref diff also rejects shadowed entry roots and leaves -o untouched.
    git(t.path(), &["init", "-q"]);
    git(t.path(), &["config", "user.name", "Public test"]);
    git(t.path(), &["config", "user.email", "test@example.invalid"]);
    git(t.path(), &["add", "v1/api.proto", "v2/api.proto"]);
    git(t.path(), &["commit", "-qm", "old"]);
    git(t.path(), &["tag", "old"]);
    fs::write(
        v2.join("api.proto"),
        "syntax = \"proto3\";\npackage demo;\nmessage M { string new_name = 1; string extra = 2; }\n",
    )
    .unwrap();
    git(t.path(), &["add", "v2/api.proto"]);
    git(t.path(), &["commit", "-qm", "new"]);
    git(t.path(), &["tag", "new"]);

    let git_out = file(&t, "git_out.md", "keep git existing");
    binary()
        .current_dir(t.path())
        .args([
            "diff",
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--spec",
            "v2/api.proto",
            "-I",
            "v1",
            "-I",
            "v2",
            "-o",
        ])
        .arg(&git_out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("shadowed"));
    assert_eq!(fs::read_to_string(&git_out).unwrap(), "keep git existing");

    // Non-shadowed Git-ref root order succeeds and diffs v2/api.proto.
    let diff_out = binary()
        .current_dir(t.path())
        .args([
            "diff",
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--spec",
            "v2/api.proto",
            "-I",
            "v2",
            "-I",
            "v1",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&diff_out).unwrap();
    assert_eq!(json["summary"]["breaking"], 0);
    assert!(json["summary"]["non_breaking"].as_u64().unwrap() >= 1);
}

#[test]
fn proto_ref_field_docs_render_in_linked_inline_split_and_skill_modes() {
    let t = tempfile::tempdir().unwrap();
    let spec = file(
        &t,
        "widgets.proto",
        r#"syntax = "proto3";
package demo.v1;

// Manages widget records.
service WidgetService {
  // Fetch a widget.
  rpc GetWidget(Widget) returns (Widget);
}

// Owner metadata.
message Owner {
  // Owner display name.
  string name = 1;
}

// Lifecycle status.
enum Status {
  STATUS_UNSPECIFIED = 0;
  // Active widget.
  STATUS_ACTIVE = 1;
}

// A stored widget.
message Widget {
  // Primary owner metadata.
  Owner owner = 1;
  // Current lifecycle state.
  Status status = 2;
  // Arbitrary key-value labels.
  map<string, string> labels = 3;
}
"#,
    );

    // Single-file default linked mode and --inline-schemas mode.
    for extra in [vec![], vec!["--inline-schemas"]] {
        let out = binary()
            .arg(&spec)
            .args(["--detail", "full", "--include-schemas", "--no-report"])
            .args(&extra)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let md = String::from_utf8(out).unwrap();
        assert!(md.contains("Primary owner metadata."), "{extra:?}:\n{md}");
        assert!(
            md.contains("Field #1; JSON name `owner`"),
            "{extra:?}:\n{md}"
        );
        assert!(md.contains("Current lifecycle state."), "{extra:?}:\n{md}");
        assert!(
            md.contains("Field #2; JSON name `status`"),
            "{extra:?}:\n{md}"
        );
        assert!(
            md.contains("Arbitrary key-value labels."),
            "{extra:?}:\n{md}"
        );
        assert!(
            md.contains("Field #3; JSON name `labels`"),
            "{extra:?}:\n{md}"
        );
        assert!(md.contains("Active widget."), "{extra:?}:\n{md}");
    }

    // HealthCheckResponse.status singular enum field retains Field #1 in linked and inline modes.
    for extra in [vec![], vec!["--inline-schemas"]] {
        let out = binary()
            .arg(fixture("health-v1.20.0.proto"))
            .args(["--detail", "full", "--include-schemas", "--no-report"])
            .args(&extra)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let md = String::from_utf8(out).unwrap();
        assert!(
            md.contains("Field #1; JSON name `status`"),
            "missing HealthCheckResponse.status field note ({extra:?}):\n{md}"
        );
        assert!(
            md.contains("Used only by the Watch method."),
            "missing ServingStatus enum value comment ({extra:?}):\n{md}"
        );
    }

    // Split and Skill tree modes keep field comments on schema pages too.
    for (name, flag, value) in [
        ("split", "--split", "endpoint"),
        ("skill", "--output-mode", "skill"),
    ] {
        let dir = t.path().canonicalize().unwrap().join(name);
        binary()
            .arg(&spec)
            .args([flag, value, "--detail", "full", "--include-schemas", "-o"])
            .arg(&dir)
            .assert()
            .success();
        let schema_dir = dir.join("schemas");
        let mut combined = String::new();
        for entry in fs::read_dir(&schema_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "md") {
                combined.push_str(&fs::read_to_string(path).unwrap());
            }
        }
        assert!(
            combined.contains("Primary owner metadata."),
            "{name}:\n{combined}"
        );
        assert!(
            combined.contains("Field #1; JSON name `owner`"),
            "{name}:\n{combined}"
        );
        assert!(
            combined.contains("Current lifecycle state."),
            "{name}:\n{combined}"
        );
        assert!(
            combined.contains("Field #2; JSON name `status`"),
            "{name}:\n{combined}"
        );
        assert!(
            combined.contains("Arbitrary key-value labels."),
            "{name}:\n{combined}"
        );
        assert!(
            combined.contains("Field #3; JSON name `labels`"),
            "{name}:\n{combined}"
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinked_protos_compile_diff_and_enforce_shadowing_identically() {
    use std::os::unix::fs::symlink;

    let t = tempfile::tempdir().unwrap();
    let shared = t.path().join("shared");
    let proto = t.path().join("proto");
    fs::create_dir_all(&shared).unwrap();
    fs::create_dir_all(proto.join("v1")).unwrap();

    // 1. Direct symlink under -I: proto/api.proto -> ../shared/api.proto
    fs::write(
        shared.join("api.proto"),
        "syntax = \"proto3\"; package demo; service Ping { rpc Check(Req) returns (Req); } message Req { string id = 1; }\n",
    )
    .unwrap();
    symlink(shared.join("api.proto"), proto.join("api.proto")).unwrap();

    binary()
        .arg(proto.join("api.proto"))
        .args(["-I"])
        .arg(&proto)
        .arg("--no-report")
        .assert()
        .success()
        .stdout(predicates::str::contains("demo.Ping.Check"));

    binary()
        .arg("diff")
        .arg(proto.join("api.proto"))
        .arg(proto.join("api.proto"))
        .args(["-I"])
        .arg(&proto)
        .args(["--format", "json"])
        .assert()
        .success();

    // 2. Subdirectory symlink under -I importing a symlinked dependency:
    // both compile (conversion) and compile_bytes (diff) preserve `v1/api.proto`
    // and produce identical neutral contracts.
    let store = t.path().join("store");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("dep.proto"),
        "syntax = \"proto3\"; package demo.v1; message Dep { string value = 1; }\n",
    )
    .unwrap();
    fs::write(
        store.join("api.proto"),
        "syntax = \"proto3\"; package demo.v1; import \"v1/dep.proto\"; service S { rpc Get(Dep) returns (Dep); }\n",
    )
    .unwrap();
    symlink(store.join("dep.proto"), proto.join("v1/dep.proto")).unwrap();
    symlink(store.join("api.proto"), proto.join("v1/api.proto")).unwrap();

    binary()
        .arg(proto.join("v1/api.proto"))
        .args(["-I"])
        .arg(&proto)
        .args(["--detail", "full", "--include-schemas", "--no-report"])
        .assert()
        .success()
        .stdout(predicates::str::contains("demo.v1.S.Get"));

    let diff_out = binary()
        .arg("diff")
        .arg(proto.join("v1/api.proto"))
        .arg(proto.join("v1/api.proto"))
        .args(["-I"])
        .arg(&proto)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&diff_out).unwrap();
    let doc = vimanam::contract::protobuf::Document::read(
        &proto.join("v1/api.proto"),
        std::slice::from_ref(&proto),
    )
    .unwrap();
    let expected_contract = serde_json::to_value(vimanam::contract::protobuf::adapt(&doc)).unwrap();
    assert_eq!(json["old"]["contract"], expected_contract);
    assert_eq!(json["new"]["contract"], expected_contract);
    let file_names: Vec<_> = json["old"]["contract"]["entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["attributes"]["kind"] == "file")
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(file_names.contains(&"v1/api.proto"), "{file_names:?}");
    assert!(file_names.contains(&"v1/dep.proto"), "{file_names:?}");

    // 3. Real shadow fails in both compile (single-file) and compile_bytes (diff),
    // leaving -o untouched.
    let v1 = t.path().join("v1");
    let v2 = t.path().join("v2");
    let other = t.path().join("other");
    fs::create_dir_all(&v1).unwrap();
    fs::create_dir_all(&v2).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::write(
        v1.join("api.proto"),
        "syntax = \"proto3\"; package demo; message M { string first = 1; }\n",
    )
    .unwrap();
    fs::write(
        other.join("api.proto"),
        "syntax = \"proto3\"; package demo; message M { string second = 1; }\n",
    )
    .unwrap();
    symlink(other.join("api.proto"), v2.join("api.proto")).unwrap();

    let single_out = file(&t, "single_out.md", "keep single");
    binary()
        .arg(v2.join("api.proto"))
        .args(["-I"])
        .arg(&v1)
        .args(["-I"])
        .arg(&v2)
        .arg("-o")
        .arg(&single_out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("shadowed"));
    assert_eq!(fs::read_to_string(&single_out).unwrap(), "keep single");

    let diff_file = file(&t, "diff_out.md", "keep diff");
    binary()
        .arg("diff")
        .arg(v2.join("api.proto"))
        .arg(v2.join("api.proto"))
        .args(["-I"])
        .arg(&v1)
        .args(["-I"])
        .arg(&v2)
        .arg("-o")
        .arg(&diff_file)
        .assert()
        .failure()
        .stderr(predicates::str::contains("shadowed"));
    assert_eq!(fs::read_to_string(&diff_file).unwrap(), "keep diff");

    // Reversing -I so v2 precedes v1 succeeds in both modes.
    binary()
        .arg(v2.join("api.proto"))
        .args(["-I"])
        .arg(&v2)
        .args(["-I"])
        .arg(&v1)
        .arg("--no-report")
        .assert()
        .success();

    binary()
        .arg("diff")
        .arg(v2.join("api.proto"))
        .arg(v2.join("api.proto"))
        .args(["-I"])
        .arg(&v2)
        .args(["-I"])
        .arg(&v1)
        .assert()
        .success();
}

#[test]
fn streaming_changes_fail_wire_gate() {
    let t = tempfile::tempdir().unwrap();
    let a = file(
        &t,
        "old.proto",
        "syntax=\"proto3\"; message M {} service S { rpc R(M) returns (M); }",
    );
    for (name, rpc) in [
        ("client.proto", "rpc R(stream M) returns (M);"),
        ("server.proto", "rpc R(M) returns (stream M);"),
    ] {
        let b = file(
            &t,
            name,
            &format!("syntax=\"proto3\"; message M {{}} service S {{ {rpc} }}"),
        );
        for scope in ["wire", "source", "all"] {
            binary()
                .arg("diff")
                .arg(&a)
                .arg(&b)
                .args(["--fail-on-breaking", "--breaking", scope])
                .assert()
                .code(3);
        }
    }
}

#[test]
fn mixed_descriptor_and_source_ignore_bundled_descriptor_drift() {
    for (old, new) in [
        ("custom-options.pb", "custom-options.proto"),
        ("custom-options.proto", "custom-options.pb"),
    ] {
        let output = binary()
            .env("PATH", "")
            .arg("diff")
            .arg(fixture(old))
            .arg(fixture(new))
            .args(["--format", "json", "--fail-on-breaking"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["changes"], Value::Array(vec![]), "{old} -> {new}");
    }

    let t = tempfile::tempdir().unwrap();
    let changed = file(
        &t,
        "custom-options.proto",
        &fs::read_to_string(fixture("custom-options.proto"))
            .unwrap()
            .replace(
                "google.protobuf.Timestamp created_at = 2",
                "string created_at = 2",
            ),
    );
    let output = binary()
        .arg("diff")
        .arg(fixture("custom-options.pb"))
        .arg(&changed)
        .args([
            "--format",
            "json",
            "--fail-on-breaking",
            "--breaking",
            "wire",
        ])
        .assert()
        .code(3)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    let changes = json["changes"].as_array().unwrap();
    assert!(
        changes
            .iter()
            .all(|c| c["native"]["full_name"] == "demo.v1.Item.created_at"),
        "{changes:#?}"
    );
    assert!(
        changes
            .iter()
            .any(|c| c["native_kind"] == "FIELD_SAME_TYPE")
    );
}
/// A `.proto` spec path that is a symlink or a submodule (gitlink) at a ref
/// is rejected with the literal-path error and exit 1, on either side, also
/// when lookups come from the full per-commit listing used for `.proto`.
#[cfg(unix)]
#[test]
fn symlink_and_submodule_proto_specs_are_not_regular_files() {
    let t = historical_fixture();
    std::os::unix::fs::symlink("api.proto", t.path().join("link.proto")).unwrap();
    git(t.path(), &["add", "link.proto"]);
    let head = String::from_utf8(git(t.path(), &["rev-parse", "HEAD"])).unwrap();
    git(
        t.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},sub.proto", head.trim()),
        ],
    );
    git(t.path(), &["commit", "-qm", "non-regular specs"]);
    let commit = String::from_utf8(git(t.path(), &["rev-parse", "HEAD"])).unwrap();
    let commit = commit.trim();
    let out = t.path().join("out.md");
    for spec in ["link.proto", "sub.proto"] {
        let expected =
            format!("Error: Git path is missing or is not a regular file at {commit}: {spec}\n");
        // As the followed --spec at --to-ref.
        binary()
            .current_dir(t.path())
            .args([
                "diff",
                "--from-ref",
                "old",
                "--to-ref",
                "HEAD",
                "--spec",
                spec,
            ])
            .args(["-I", "deps", "-o"])
            .arg(&out)
            .assert()
            .code(1)
            .stdout("")
            .stderr(expected.clone());
        // As a literal --from-spec at --from-ref.
        binary()
            .current_dir(t.path())
            .args(["diff", "--from-ref", "HEAD", "--to-ref", "old"])
            .args([
                "--from-spec",
                spec,
                "--to-spec",
                "api.proto",
                "-I",
                "deps",
                "-o",
            ])
            .arg(&out)
            .assert()
            .code(1)
            .stdout("")
            .stderr(expected);
        assert!(!out.exists());
    }
}

/// Writes `text` at `root/relative`, creating parent directories.
fn write_proto(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

const SIDE_ENTRY: &str = "syntax = \"proto3\";\npackage svc;\nimport \"common/types.proto\";\nservice S { rpc Get(common.T) returns (common.T); }\n";
const SIDE_TYPES_OLD: &str = "syntax = \"proto3\";\npackage common;\nmessage T { string a = 1; }\n";
const SIDE_TYPES_NEW: &str = "syntax = \"proto3\";\npackage common;\nmessage T { int64 a = 1; }\n";

fn side_diff_json(args: &[&std::ffi::OsStr]) -> Value {
    let output = binary()
        .env("PATH", "")
        .arg("diff")
        .args(args)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).unwrap()
}

/// #162: a change inside a transitively imported file is seen only when each
/// side resolves imports against its own roots. The shared `-I` behaviour is
/// unchanged: one root serves both sides, so the new side's import silently
/// resolves to the old tree's file.
#[test]
fn per_side_proto_paths_see_changes_in_imported_files() {
    let t = tempfile::tempdir().unwrap();
    let old_root = t.path().join("old/proto");
    let new_root = t.path().join("new/proto");
    write_proto(&old_root, "svc/api.proto", SIDE_ENTRY);
    write_proto(&old_root, "common/types.proto", SIDE_TYPES_OLD);
    write_proto(&new_root, "svc/api.proto", SIDE_ENTRY);
    write_proto(&new_root, "common/types.proto", SIDE_TYPES_NEW);
    let old = old_root.join("svc/api.proto");
    let new = new_root.join("svc/api.proto");

    // Shared -I: both sides import old/proto/common/types.proto.
    let shared = side_diff_json(&[
        old.as_os_str(),
        new.as_os_str(),
        "-I".as_ref(),
        old_root.as_os_str(),
    ]);
    assert_eq!(shared["summary"]["breaking"], 0, "{shared:#}");
    assert!(
        shared["changes"].as_array().unwrap().is_empty(),
        "{shared:#}"
    );

    // Per-side roots: the field type change in the imported file is breaking.
    let args = [
        old.as_os_str(),
        new.as_os_str(),
        "--old-proto-path".as_ref(),
        old_root.as_os_str(),
        "--new-proto-path".as_ref(),
        new_root.as_os_str(),
    ];
    let per_side = side_diff_json(&args);
    assert!(
        per_side["summary"]["breaking"].as_u64().unwrap() >= 1,
        "{per_side:#}"
    );
    assert!(
        per_side.to_string().contains("common.T"),
        "change names the imported message: {per_side:#}"
    );
    binary()
        .arg("diff")
        .args(args)
        .arg("--fail-on-breaking")
        .assert()
        .code(3);

    // A shared -I alongside both per-side lists is not consulted.
    let with_shared = side_diff_json(&[
        old.as_os_str(),
        new.as_os_str(),
        "-I".as_ref(),
        old_root.as_os_str(),
        "--old-proto-path".as_ref(),
        old_root.as_os_str(),
        "--new-proto-path".as_ref(),
        new_root.as_os_str(),
    ]);
    assert_eq!(with_shared["changes"], per_side["changes"]);
}

/// Each side's imports resolve only against its own roots: an import missing
/// from the old tree fails to compile rather than being read from the new
/// tree, even when a shared -I names the new tree.
#[test]
fn per_side_proto_paths_do_not_cross_sides() {
    let t = tempfile::tempdir().unwrap();
    let old_root = t.path().join("old/proto");
    let new_root = t.path().join("new/proto");
    write_proto(&old_root, "svc/api.proto", SIDE_ENTRY);
    write_proto(&new_root, "svc/api.proto", SIDE_ENTRY);
    write_proto(&new_root, "common/types.proto", SIDE_TYPES_NEW);
    let out = file(&t, "out.md", "keep existing");
    binary()
        .arg("diff")
        .arg(old_root.join("svc/api.proto"))
        .arg(new_root.join("svc/api.proto"))
        .arg("-I")
        .arg(&new_root)
        .arg("--old-proto-path")
        .arg(&old_root)
        .arg("--new-proto-path")
        .arg(&new_root)
        .arg("-o")
        .arg(&out)
        .assert()
        .code(1)
        .stderr(predicates::str::contains("common/types.proto"));
    assert_eq!(fs::read_to_string(&out).unwrap(), "keep existing");
}

/// The two sides may use different layouts and root counts, and a side
/// without its own flag keeps using the shared -I roots.
#[test]
fn per_side_proto_paths_allow_different_layouts() {
    let t = tempfile::tempdir().unwrap();
    // Old: a flat tree with one root.
    let old_root = t.path().join("old");
    write_proto(&old_root, "api.proto", SIDE_ENTRY);
    write_proto(&old_root, "common/types.proto", SIDE_TYPES_OLD);
    // New: sources and vendored imports under two separate roots.
    let new_src = t.path().join("new/src/proto");
    let new_vendor = t.path().join("new/third_party");
    write_proto(&new_src, "svc/api.proto", SIDE_ENTRY);
    write_proto(&new_vendor, "common/types.proto", SIDE_TYPES_NEW);
    let old = old_root.join("api.proto");
    let new = new_src.join("svc/api.proto");

    let json = side_diff_json(&[
        old.as_os_str(),
        new.as_os_str(),
        "-I".as_ref(),
        old_root.as_os_str(),
        "--new-proto-path".as_ref(),
        new_src.as_os_str(),
        "--new-proto-path".as_ref(),
        new_vendor.as_os_str(),
    ]);
    assert!(
        json["summary"]["breaking"].as_u64().unwrap() >= 1,
        "{json:#}"
    );
    assert!(json.to_string().contains("common.T"), "{json:#}");
}

/// Per-side roots are rejected with exit 1, before any output is created, in
/// Git-ref mode and for inputs that are not `.proto` sources.
#[test]
fn per_side_proto_paths_reject_git_refs_and_non_proto_inputs() {
    let t = tempfile::tempdir().unwrap();
    let out = file(&t, "out.md", "keep existing");
    for flag in ["--old-proto-path", "--new-proto-path"] {
        binary()
            .current_dir(t.path())
            .args(["diff", "--from-ref", "a", "--to-ref", "b", "--spec"])
            .arg("api.proto")
            .arg(flag)
            .arg(t.path())
            .arg("-o")
            .arg(&out)
            .assert()
            .code(1)
            .stderr(predicates::str::contains("file-pair diffs only"));
    }

    let openapi = file(
        &t,
        "openapi.json",
        r#"{"openapi":"3.0.0","info":{"title":"t","version":"1"},"paths":{}}"#,
    );
    let sdl = file(&t, "schema.graphql", "type Query { a: String }\n");
    let proto = file(&t, "api.proto", "syntax = \"proto3\";\nmessage M {}\n");
    let descriptor = fixture("health-v1.0.0.pb");
    for (old, new) in [
        (&openapi, &openapi),
        (&sdl, &sdl),
        (&descriptor, &descriptor),
        (&proto, &descriptor),
    ] {
        binary()
            .arg("diff")
            .arg(old)
            .arg(new)
            .arg("--old-proto-path")
            .arg(t.path())
            .arg("-o")
            .arg(&out)
            .assert()
            .code(1)
            .stderr(predicates::str::contains("require .proto source files"));
    }
    assert_eq!(fs::read_to_string(&out).unwrap(), "keep existing");
}
