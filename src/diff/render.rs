// ── rendering ───────────────────────────────────────────────────────────────

use std::io::Write;

use anyhow::Result;
use serde_json::Value;

use indexmap::{IndexMap, IndexSet};

use super::{
    Change, ChangeKind, ComponentRef, Deltas, Location, MoveRule, SchemaUse, Severity, SpecDiff,
    UseLocation, ValueChange, ValueChangeKind, locate, severity,
};

/// Writes the diff as Markdown: a title, a one-line summary, one table per
/// severity that has entries (breaking, non-breaking, needs review), an
/// `## Affected operations` section listing each changed shared component's
/// uses once, and the `## Deltas` section when `deltas` is given.
pub fn write_diff<W: Write>(
    writer: &mut W,
    diff: &SpecDiff,
    deltas: Option<&Deltas>,
) -> Result<()> {
    let title = if diff.old_title == diff.new_title {
        diff.old_title.clone()
    } else {
        format!("{} → {}", diff.old_title, diff.new_title)
    };
    writeln!(
        writer,
        "# API Diff: {title} {} → {}",
        diff.old_version, diff.new_version
    )?;
    writeln!(writer)?;

    if diff.changes.is_empty() {
        writeln!(writer, "**Summary:** No changes.")?;
    } else {
        // "N moved" appears only when moves were matched, so exact-path
        // reports keep their established summary line.
        let moved = match diff.endpoints_moved() {
            0 => String::new(),
            n => format!(", {n} moved"),
        };
        writeln!(
            writer,
            "**Summary:** {} added, {} removed{moved}, {} changed; {} breaking, {} non-breaking, {} to review",
            pluralize(diff.endpoints_added(), "endpoint"),
            diff.endpoints_removed(),
            diff.endpoints_changed(),
            diff.count(Severity::Breaking),
            diff.count(Severity::NonBreaking),
            diff.count(Severity::Review),
        )?;

        let sections = [
            (Severity::Breaking, "Breaking changes"),
            (Severity::NonBreaking, "Non-breaking changes"),
            (Severity::Review, "Needs review"),
        ];
        for (level, heading) in sections {
            let rows: Vec<&Change> = diff
                .changes
                .iter()
                .filter(|change| severity(change) == level)
                .collect();
            if rows.is_empty() {
                continue;
            }

            writeln!(writer)?;
            writeln!(writer, "## {heading} ({})", rows.len())?;
            writeln!(writer)?;
            writeln!(writer, "| Change | Endpoint | Detail |")?;
            writeln!(writer, "|--------|----------|--------|")?;
            for change in rows {
                let subject = match &change.kind {
                    ChangeKind::ComponentSchemaChanged { component, .. } => {
                        format!("component {}", component_name(component))
                    }
                    _ => format!("`{}`", change.endpoint),
                };
                writeln!(
                    writer,
                    "| {} | {} | {} |",
                    change_label(&change.kind),
                    escape_cell(&subject),
                    escape_cell(&change_detail(&change.kind)),
                )?;
            }
        }
        write_affected_operations(writer, diff)?;
    }

    if let Some(deltas) = deltas {
        writeln!(writer)?;
        writeln!(writer, "## Deltas")?;
        writeln!(writer)?;
        writeln!(writer, "| Check | Old | New | Δ |")?;
        writeln!(writer, "|-------|----:|----:|--:|")?;
        for (label, old_count, new_count) in &deltas.hygiene {
            writeln!(
                writer,
                "| {label} | {old_count} | {new_count} | {} |",
                signed_delta(*old_count, *new_count)
            )?;
        }
        writeln!(writer)?;
        writeln!(
            writer,
            "Token estimate (--detail full --include-schemas): {} → {} ({})",
            deltas.tokens_old,
            deltas.tokens_new,
            signed_delta(deltas.tokens_old, deltas.tokens_new)
        )?;
    }

    Ok(())
}

/// `` `Widget` ``, or `` `Pet` → `Dog` `` for a retargeted reference.
fn component_name(component: &ComponentRef) -> String {
    if component.renamed() {
        format!("`{}` → `{}`", component.old, component.new)
    } else {
        format!("`{}`", component.new)
    }
}

/// Lists, once per changed shared component (in report order), every
/// operation schema that reaches it. Omitted when no component changed.
fn write_affected_operations<W: Write>(writer: &mut W, diff: &SpecDiff) -> Result<()> {
    let mut components: IndexMap<&ComponentRef, IndexSet<&SchemaUse>> = IndexMap::new();
    for change in &diff.changes {
        if let ChangeKind::ComponentSchemaChanged {
            component, uses, ..
        } = &change.kind
        {
            components.entry(component).or_default().extend(uses);
        }
    }
    if components.is_empty() {
        return Ok(());
    }
    writeln!(writer)?;
    writeln!(writer, "## Affected operations")?;
    for (component, uses) in components {
        writeln!(writer)?;
        writeln!(writer, "### {}", component_name(component))?;
        writeln!(writer)?;
        for use_ in uses {
            writeln!(
                writer,
                "- `{}` {}",
                use_.endpoint,
                use_label(&use_.location)
            )?;
        }
    }
    Ok(())
}

/// `response 200`, `request body`, `parameter `limit` (query)`.
fn use_label(location: &UseLocation) -> String {
    match location {
        UseLocation::Parameter { name, location } => format!("parameter `{name}` ({location})"),
        UseLocation::RequestBody { .. } => "request body".to_string(),
        UseLocation::Response { status } => format!("response {status}"),
    }
}

/// `+3`, `-1` or `0`.
fn signed_delta(old: usize, new: usize) -> String {
    let delta = new as i64 - old as i64;
    if delta > 0 {
        format!("+{delta}")
    } else {
        delta.to_string()
    }
}

/// `1 endpoint`, `2 endpoints`.
fn pluralize(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Escapes the characters that would break a Markdown table cell.
fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br/>")
}

/// The `Change` column.
fn change_label(kind: &ChangeKind) -> &'static str {
    match kind {
        ChangeKind::EndpointAdded => "Endpoint added",
        ChangeKind::EndpointRemoved { .. } => "Endpoint removed",
        ChangeKind::EndpointMoved { .. } => "Endpoint moved",
        ChangeKind::ParameterAdded { .. } => "Parameter added",
        ChangeKind::ParameterRemoved { .. } => "Parameter removed",
        ChangeKind::ParameterRequiredChanged {
            now_required: true, ..
        } => "Parameter newly required",
        ChangeKind::ParameterRequiredChanged {
            now_required: false,
            ..
        } => "Parameter made optional",
        ChangeKind::ParameterLocationChanged { .. } => "Parameter location changed",
        ChangeKind::ParameterSchemaChanged { .. } => "Parameter schema changed",
        ChangeKind::ResponseAdded { .. } => "Response added",
        ChangeKind::ResponseRemoved { .. } => "Response removed",
        ChangeKind::OperationIdChanged { .. } => "operationId changed",
        ChangeKind::DeprecatedChanged { now: true } => "Marked deprecated",
        ChangeKind::DeprecatedChanged { now: false } => "Deprecation removed",
        ChangeKind::RequestSchemaChanged { .. } => "Request schema changed",
        ChangeKind::ResponseSchemaChanged { .. } => "Response schema changed",
        ChangeKind::ComponentSchemaChanged { .. } => "Component schema changed",
    }
}

/// The `Detail` column.
fn change_detail(kind: &ChangeKind) -> String {
    match kind {
        ChangeKind::EndpointAdded => "-".to_string(),
        ChangeKind::EndpointRemoved { was_deprecated } => if *was_deprecated {
            "was deprecated"
        } else {
            "-"
        }
        .to_string(),
        ChangeKind::EndpointMoved {
            from, matched_by, ..
        } => format!(
            "from `{from}` (matched by {})",
            match matched_by {
                MoveRule::OperationId => "operationId",
                MoveRule::VersionSegment => "version segment",
            }
        ),
        ChangeKind::ParameterAdded {
            name,
            location,
            required,
        } => format!(
            "`{name}` ({location}), {}",
            if *required { "required" } else { "optional" }
        ),
        ChangeKind::ParameterRemoved { name, location }
        | ChangeKind::ParameterRequiredChanged { name, location, .. } => {
            format!("`{name}` ({location})")
        }
        ChangeKind::ParameterLocationChanged {
            name,
            old_location,
            new_location,
        } => format!("`{name}` {old_location} → {new_location}"),
        ChangeKind::ParameterSchemaChanged {
            name,
            location,
            change,
        } => format!("`{name}` ({location}) {}", describe_value_change(change)),
        ChangeKind::ResponseAdded { status } | ChangeKind::ResponseRemoved { status } => {
            status.clone()
        }
        ChangeKind::OperationIdChanged { old, new } => {
            format!("{} → {}", operation_id(old), operation_id(new))
        }
        ChangeKind::DeprecatedChanged { .. } => "-".to_string(),
        ChangeKind::RequestSchemaChanged { change } => describe_value_change(change),
        ChangeKind::ResponseSchemaChanged { status, change } => {
            format!("{status} {}", describe_value_change(change))
        }
        ChangeKind::ComponentSchemaChanged {
            direction,
            change,
            negated,
            uses,
            ..
        } => {
            let operations = uses
                .iter()
                .map(|use_| &use_.endpoint)
                .collect::<IndexSet<_>>()
                .len();
            format!(
                "{}{} {} ({})",
                direction.as_str(),
                if *negated { " under `not`" } else { "" },
                describe_value_change(change),
                pluralize(operations, "operation")
            )
        }
    }
}

fn operation_id(id: &Option<String>) -> String {
    match id {
        Some(id) => format!("`{id}`"),
        None => "(none)".to_string(),
    }
}

/// A short rendering of a JSON value for the detail column: bare strings,
/// compact JSON for everything else, truncated when long.
fn brief(value: &Value) -> String {
    const MAX_CHARS: usize = 60;
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if text.chars().count() > MAX_CHARS {
        let cut: String = text.chars().take(MAX_CHARS).collect();
        format!("{cut}…")
    } else {
        text
    }
}

fn display_pointer(pointer: &str) -> &str {
    if pointer.is_empty() { "/" } else { pointer }
}

/// Human wording for one value change, e.g.
/// `` `/properties/pricing` added to `required` `` or
/// `` `/type` changed `string` → `integer` ``.
fn describe_value_change(change: &ValueChange) -> String {
    use ValueChangeKind::{Added, Changed, Removed};

    let pointer = &change.pointer;
    match (locate(pointer), &change.kind) {
        (Location::RequiredElement, Added(_)) => {
            format!("`{}` added to `required`", required_member_pointer(pointer))
        }
        (Location::RequiredElement, Removed(_)) => {
            format!(
                "`{}` removed from `required`",
                required_member_pointer(pointer)
            )
        }
        (Location::EnumElement, Added(value)) => {
            format!(
                "`{}` value `{}` added",
                parent_pointer(pointer),
                brief(value)
            )
        }
        (Location::EnumElement, Removed(value)) => {
            format!(
                "`{}` value `{}` removed",
                parent_pointer(pointer),
                brief(value)
            )
        }
        (_, Added(value)) if !value.is_object() && !value.is_array() => {
            format!("`{}` added (`{}`)", display_pointer(pointer), brief(value))
        }
        (_, Added(_)) => format!("`{}` added", display_pointer(pointer)),
        (_, Removed(_)) => format!("`{}` removed", display_pointer(pointer)),
        (_, Changed { old, .. }) if pointer.is_empty() && old.is_null() => {
            "schema added".to_string()
        }
        (_, Changed { new, .. }) if pointer.is_empty() && new.is_null() => {
            "schema removed".to_string()
        }
        (_, Changed { old, new }) => format!(
            "`{}` changed `{}` → `{}`",
            display_pointer(pointer),
            brief(old),
            brief(new)
        ),
    }
}

/// `/x/required/name` → `/x/properties/name`: names the property a `required`
/// membership change is about.
pub(super) fn required_member_pointer(pointer: &str) -> String {
    match pointer.rsplit_once('/') {
        Some((set, member)) => {
            let base = set.strip_suffix("/required").unwrap_or(set);
            format!("{base}/properties/{member}")
        }
        None => pointer.to_string(),
    }
}

fn parent_pointer(pointer: &str) -> &str {
    pointer
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or(pointer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::test_support::{added, changed, removed};
    use serde_json::json;

    // ── rendering helpers ────────────────────────────────────────────────

    #[test]
    fn describe_value_change_wording() {
        assert_eq!(
            describe_value_change(&added("/required/pricing", json!("pricing"))),
            "`/properties/pricing` added to `required`"
        );
        assert_eq!(
            describe_value_change(&removed("/items/required/id", json!("id"))),
            "`/items/properties/id` removed from `required`"
        );
        assert_eq!(
            describe_value_change(&removed(
                "/properties/status/enum/archived",
                json!("archived")
            )),
            "`/properties/status/enum` value `archived` removed"
        );
        assert_eq!(
            describe_value_change(&changed("/type", json!("string"), json!("integer"))),
            "`/type` changed `string` → `integer`"
        );
        assert_eq!(
            describe_value_change(&added("/properties/pricing", json!({"type": "object"}))),
            "`/properties/pricing` added"
        );
        assert_eq!(
            describe_value_change(&added("/minimum", json!(0))),
            "`/minimum` added (`0`)"
        );
        assert_eq!(
            describe_value_change(&changed("", Value::Null, json!({}))),
            "schema added"
        );
    }

    #[test]
    fn signed_delta_formats_sign() {
        assert_eq!(signed_delta(3, 5), "+2");
        assert_eq!(signed_delta(5, 3), "-2");
        assert_eq!(signed_delta(4, 4), "0");
    }

    #[test]
    fn write_diff_with_no_changes_says_so() {
        let diff = SpecDiff {
            old_title: "API".into(),
            old_version: "1".into(),
            new_title: "API".into(),
            new_version: "2".into(),
            changes: Vec::new(),
        };
        let mut buffer = Vec::new();
        write_diff(&mut buffer, &diff, None).unwrap();
        assert_eq!(
            String::from_utf8(buffer).unwrap(),
            "# API Diff: API 1 → 2\n\n**Summary:** No changes.\n"
        );
    }

    #[test]
    fn write_diff_shows_both_titles_when_they_differ() {
        let diff = SpecDiff {
            old_title: "Old".into(),
            old_version: "1".into(),
            new_title: "New".into(),
            new_version: "2".into(),
            changes: Vec::new(),
        };
        let mut buffer = Vec::new();
        write_diff(&mut buffer, &diff, None).unwrap();
        assert!(
            String::from_utf8(buffer)
                .unwrap()
                .starts_with("# API Diff: Old → New 1 → 2\n")
        );
    }
}
