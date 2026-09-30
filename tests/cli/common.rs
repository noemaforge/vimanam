use assert_cmd::Command;

pub(crate) const OAS3: &str = "tests/fixtures/petstore_oas3.json";
pub(crate) const OAS2: &str = "tests/fixtures/petstore_oas2.json";
pub(crate) const OAS3_SCHEMA_REFS: &str = "tests/fixtures/schema_refs_oas3.json";

pub(crate) const MULTI_TAG: &str = "tests/fixtures/multi_tag_oas3.json";

// A spec that trips every hygiene check at least once.
pub(crate) const HYGIENE: &str = "tests/fixtures/hygiene_oas3.json";

// Two versions of one spec. `new` adds a required `pricing` property to the
// shared `Widget` schema while leaving the `GET /widgets` operation object
// byte-identical, removes the deprecated `GET /legacy`, adds
// `GET /widgets/{id}/history`, makes the `fields` query parameter required and
// drops the 404 response on `GET /widgets/{id}`, renames and deprecates the
// `DELETE /widgets/{id}` operation, changes `WidgetInput.weight` from float
// to double, and rewords descriptions only (the `X-Trace` header, the
// `WidgetInput.name` property).
pub(crate) const DIFF_OLD: &str = "tests/fixtures/diff_old_oas3.json";
pub(crate) const DIFF_NEW: &str = "tests/fixtures/diff_new_oas3.json";

pub(crate) fn vimanam() -> Command {
    Command::cargo_bin("vimanam").unwrap()
}

/// Runs `vimanam diff` with `args` and returns `(stdout, exit code)`.
pub(crate) fn diff_run(args: &[&str]) -> (String, i32) {
    let output = vimanam().arg("diff").args(args).output().unwrap();
    (
        String::from_utf8(output.stdout).unwrap(),
        output.status.code().unwrap(),
    )
}

pub(crate) fn load_json(path: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Writes `spec` to `<dir>/<name>` so an edited copy of a fixture can be diffed.
pub(crate) fn write_spec(dir: &tempfile::TempDir, name: &str, spec: &serde_json::Value) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, serde_json::to_string_pretty(spec).unwrap()).unwrap();
    path.to_str().unwrap().to_string()
}
