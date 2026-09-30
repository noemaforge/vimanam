use crate::common::HYGIENE;
use crate::common::MULTI_TAG;
use crate::common::OAS3;
use crate::common::vimanam;
use predicates::prelude::*;

// --- Spec hygiene report (#44) ---

// One operation whose undescribed requestBody offers two media types.
const HYGIENE_MULTI_BODY: &str = "tests/fixtures/hygiene_multi_body_oas3.json";

// The report is appended after the body by default, separated by a rule.
#[test]
fn hygiene_report_is_appended_by_default() {
    vimanam()
        .arg(OAS3)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\n---\n\n## Spec Hygiene Report\n",
        ))
        .stdout(predicate::str::contains(
            "**4 endpoints** across **2 services**",
        ))
        .stdout(predicate::str::contains("| Deprecated | 1 |"))
        .stdout(predicate::str::contains(
            "### Deprecated (1)\n- `GET /store/orders`\n",
        ));
}

#[test]
fn no_report_suppresses_hygiene_report() {
    vimanam()
        .arg(OAS3)
        .arg("--no-report")
        .assert()
        .success()
        .stdout(predicate::str::contains("# Petstore API"))
        .stdout(predicate::str::contains("Spec Hygiene Report").not())
        .stdout(predicate::str::contains("\n---\n").not());
}

// Every check fires on the hygiene fixture; the whole report is asserted
// byte-for-byte so its shape (table order, list order, list item format) is
// pinned down.
#[test]
fn hygiene_report_lists_every_check() {
    let output = String::from_utf8(
        vimanam()
            .arg(HYGIENE)
            .args(["--detail", "standard"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    // A plain multi-line literal (no `\` continuations) so the two-space
    // indentation of the nested duplicate-operationId items is preserved.
    let expected = "
---

## Spec Hygiene Report

**6 endpoints** across **2 services**

| Check | Count |
|-------|------:|
| Missing description | 1 |
| Missing operationId | 1 |
| No responses documented | 1 |
| Deprecated | 2 |
| Untagged (no service tag) | 1 |
| Duplicate operationIds | 1 |
| Parameters without description | 3 |

### Missing description (1)
- `GET /health`

### Missing operationId (1)
- `GET /health`

### No responses documented (1)
- `POST /users`

### Deprecated (2)
- `GET /ping`
- `DELETE /users/{id}`

### Untagged (no service tag) (1)
- `GET /health`

### Duplicate operationIds (1)
- `getUser`
  - `DELETE /users/{id}`
  - `GET /users/{id}`

### Parameters without description (3)
- `GET /users` — `limit`
- `POST /users` — `requestBody`
- `DELETE /users/{id}` — `id`
";

    assert!(
        output.ends_with(expected),
        "report did not match.\nexpected tail:\n{expected}\nactual output:\n{output}"
    );
    // The body precedes the report.
    assert!(output.starts_with("# Hygiene API\n"), "{output}");
}

// The report analyzes the same endpoint set the body rendered: a service
// filter narrows both the counts and the service total.
#[test]
fn hygiene_report_respects_service_filter() {
    vimanam()
        .arg(HYGIENE)
        .args(["--service-filter", "Health"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "**1 endpoint** across **1 service**",
        ))
        .stdout(predicate::str::contains("| Deprecated | 1 |"))
        .stdout(predicate::str::contains(
            "| Untagged (no service tag) | 0 |",
        ))
        .stdout(predicate::str::contains("| Duplicate operationIds | 0 |"))
        .stdout(predicate::str::contains(
            "### Deprecated (1)\n- `GET /ping`\n",
        ))
        .stdout(predicate::str::contains("GET /health").not());
}

// `--exclude-deprecated` removes the deprecated DELETE, which also dissolves
// the duplicate operationId pair and drops its undescribed parameter; the
// deprecated GET /ping was the only Health endpoint, so one service remains.
#[test]
fn hygiene_report_respects_exclude_deprecated() {
    vimanam()
        .arg(HYGIENE)
        .arg("--exclude-deprecated")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "**4 endpoints** across **1 service**",
        ))
        .stdout(predicate::str::contains("| Deprecated | 0 |"))
        .stdout(predicate::str::contains("| Duplicate operationIds | 0 |"))
        .stdout(predicate::str::contains(
            "| Parameters without description | 2 |",
        ))
        .stdout(predicate::str::contains("### Deprecated").not())
        .stdout(predicate::str::contains("### Duplicate operationIds").not());
}

// `--max-tokens` budgets the body only; the report is still appended.
#[test]
fn hygiene_report_is_appended_under_max_tokens() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "full", "--max-tokens", "40"])
        .assert()
        .success()
        .stderr(predicate::str::contains("token budget"))
        .stdout(predicate::str::contains("## Spec Hygiene Report"));
}

// The file output path appends the report just like stdout does.
#[test]
fn hygiene_report_is_written_to_output_file() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.md");

    vimanam()
        .arg(HYGIENE)
        .args(["-o", out_path.to_str().unwrap()])
        .assert()
        .success();

    let content = std::fs::read_to_string(&out_path).unwrap();
    assert!(content.starts_with("# Hygiene API\n"), "{content}");
    assert!(
        content.contains("\n---\n\n## Spec Hygiene Report\n"),
        "{content}"
    );
    assert!(
        content.contains("| Duplicate operationIds | 1 |"),
        "{content}"
    );
}

// The report is deterministic even with `--sort none` (spec order), where
// nothing but stable collections keeps the lists in a fixed order.
#[test]
fn hygiene_report_is_deterministic() {
    let run = || {
        vimanam()
            .arg(HYGIENE)
            .args(["--detail", "full", "--sort", "none"])
            .output()
            .unwrap()
            .stdout
    };

    let first = run();
    for _ in 0..4 {
        assert_eq!(first, run(), "report differed between identical runs");
    }
}

// The parser emits one synthetic body parameter per requestBody media type,
// all sharing the request body's description; an undescribed body is reported
// once per endpoint, not once per media type.
#[test]
fn hygiene_report_counts_multi_media_type_body_once() {
    let output = String::from_utf8(
        vimanam()
            .arg(HYGIENE_MULTI_BODY)
            .args(["--detail", "standard"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    // The body still documents both media types...
    assert!(
        output.contains("`requestBody (application/json)` | body |"),
        "{output}"
    );
    assert!(
        output.contains("`requestBody (application/xml)` | body |"),
        "{output}"
    );
    // ...but the report flags the body once.
    assert!(
        output.contains("| Parameters without description | 1 |"),
        "{output}"
    );
    assert!(
        output.ends_with(
            "### Parameters without description (1)\n- `POST /items` — `requestBody (application/json)`\n"
        ),
        "{output}"
    );
    assert_eq!(
        output.matches("- `POST /items` — `requestBody").count(),
        1,
        "{output}"
    );
}

// Exactly one blank line separates the body from the report at every detail
// level, even though the views themselves end with differing trailing
// whitespace (one newline at summary, a blank line otherwise).
#[test]
fn hygiene_report_is_separated_from_body_by_one_blank_line() {
    for detail in ["summary", "basic", "standard", "full"] {
        vimanam()
            .arg(OAS3)
            .args(["--detail", detail])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "\n\n---\n\n## Spec Hygiene Report",
            ))
            .stdout(predicate::str::contains("\n\n\n---").not());
    }
}

// A multi-tag operation is rendered under each of its services but analyzed
// once; the service count still reflects every service it appears in.
#[test]
fn hygiene_report_counts_multi_tag_endpoint_once() {
    vimanam()
        .arg(MULTI_TAG)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "**1 endpoint** across **2 services**",
        ));
}

// Filters that leave nothing visible produce an all-zero report with no
// detail sections, and no services (only services with visible endpoints
// are counted).
#[test]
fn hygiene_report_on_empty_filtered_set_is_all_zero() {
    let output = String::from_utf8(
        vimanam()
            .arg(OAS3)
            .args(["--path-filter", "/nope"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    let expected = "\
## Spec Hygiene Report

**0 endpoints** across **0 services**

| Check | Count |
|-------|------:|
| Missing description | 0 |
| Missing operationId | 0 |
| No responses documented | 0 |
| Deprecated | 0 |
| Untagged (no service tag) | 0 |
| Duplicate operationIds | 0 |
| Parameters without description | 0 |
";
    assert!(output.ends_with(expected), "{output}");
    assert!(!output.contains("\n### "), "{output}");
}
