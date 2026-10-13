use crate::common::vimanam;
use serde_json::Value;
const OLD: &str = "tests/fixtures/diff_old_oas3.json";
const NEW: &str = "tests/fixtures/diff_new_oas3.json";
fn run(extra: &[&str]) -> std::process::Output {
    vimanam()
        .args(["diff", OLD, NEW, "--format", "json"])
        .args(extra)
        .output()
        .unwrap()
}
#[test]
fn default_is_v2_and_explicit_v2_matches_bytes() {
    let default = run(&[]);
    let explicit = run(&["--contract", "2"]);
    assert_eq!(default.stdout, explicit.stdout);
    let document: Value = serde_json::from_slice(&default.stdout).unwrap();
    assert_eq!(document["schema_version"], 2);
    assert!(
        document["new"]["contract"]["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "type")
    );
    assert!(
        document["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["id"].as_str().unwrap().starts_with("vc2_"))
    );
    let breaking = document["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["severity"] == "breaking")
        .count();
    assert_eq!(document["summary"]["breaking"], breaking);
    let gated = run(&["--fail-on-breaking"]);
    assert_eq!(gated.status.code(), Some(if breaking > 0 { 3 } else { 0 }));
    assert_eq!(default.stdout, gated.stdout);
}
#[test]
fn explicit_v1_retains_legacy_envelope_and_ids() {
    let output = run(&["--contract", "1"]);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema_version"], 1);
    assert!(
        document["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["id"].as_str().unwrap().starts_with("vc1_"))
    );
}
#[test]
fn invalid_contract_is_usage_error_and_report_file_matches_stdout() {
    assert_eq!(run(&["--contract", "3"]).status.code(), Some(2));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diff.json");
    let file = run(&["--report", "-o", path.to_str().unwrap()]);
    assert!(file.status.success());
    let stdout = run(&["--report"]);
    assert_eq!(std::fs::read(path).unwrap(), stdout.stdout);
    let document: Value = serde_json::from_slice(&stdout.stdout).unwrap();
    assert!(document["deltas"].is_object());
}
