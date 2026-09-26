//! Diagnostics for the exact operation selector (`--operation`,
//! `--operation-id`). The selection itself is applied as one more filter in the
//! views (see [`OperationSelector::matches`]); this module reports selectors
//! that matched nothing in the spec (an error) and selectors whose operations
//! another filter removed (a warning).

use crate::markdown::removing_filters;
use crate::models::{ApiDocumentation, DocConfig, Endpoint, OperationSelector};

/// The error message for every selector value that matches no endpoint in the
/// whole spec, before any other filter is applied, or `None` when all of them
/// match. Values are listed per flag in the order given; `--operation` values
/// carry a "did you mean" suggestion when a close enough endpoint exists.
pub fn unmatched_selectors(doc: &ApiDocumentation, selector: &OperationSelector) -> Option<String> {
    let mut lines = Vec::new();

    let unmatched_ops: Vec<String> = selector
        .operations
        .iter()
        .filter(|op| !doc.endpoints.iter().any(|e| op.matches(e)))
        .map(|op| {
            let wanted = op.to_string();
            let candidates = doc
                .endpoints
                .iter()
                .map(|e| format!("{} {}", e.method, e.path));
            match suggest(&wanted, candidates) {
                Some(hint) => format!("{wanted:?} (did you mean {hint:?}?)"),
                None => format!("{wanted:?}"),
            }
        })
        .collect();
    if !unmatched_ops.is_empty() {
        lines.push(format!(
            "--operation matched no endpoint: {}",
            unmatched_ops.join(", ")
        ));
    }

    let unmatched_ids: Vec<String> = selector
        .operation_ids
        .iter()
        .filter(|id| {
            !doc.endpoints
                .iter()
                .any(|e| e.operation_id.as_ref() == Some(*id))
        })
        .map(|id| {
            let candidates = doc.endpoints.iter().filter_map(|e| e.operation_id.clone());
            match suggest(id, candidates) {
                Some(hint) => format!("{id:?} (did you mean {hint:?}?)"),
                None => format!("{id:?}"),
            }
        })
        .collect();
    if !unmatched_ids.is_empty() {
        lines.push(format!(
            "--operation-id matched no endpoint: {}",
            unmatched_ids.join(", ")
        ));
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// One stderr warning per selector value that matched endpoints in the spec
/// but none that survive the other filters, naming the filters that removed
/// them. Values that matched nothing at all are the error case
/// ([`unmatched_selectors`]) and are skipped here.
pub fn filtered_out_warnings(doc: &ApiDocumentation, config: &DocConfig) -> Vec<String> {
    let Some(selector) = &config.operation_selector else {
        return Vec::new();
    };

    let ops = selector.operations.iter().map(|op| {
        let matched: Vec<&Endpoint> = doc.endpoints.iter().filter(|e| op.matches(e)).collect();
        (format!("--operation {:?}", op.to_string()), matched)
    });
    let ids = selector.operation_ids.iter().map(|id| {
        let matched: Vec<&Endpoint> = doc
            .endpoints
            .iter()
            .filter(|e| e.operation_id.as_ref() == Some(id))
            .collect();
        (format!("--operation-id {id:?}"), matched)
    });

    ops.chain(ids)
        .filter_map(|(label, matched)| {
            if matched.is_empty() {
                return None;
            }
            let mut removed_by: Vec<&str> = Vec::new();
            for endpoint in &matched {
                let filters = removing_filters(endpoint, config);
                if filters.is_empty() {
                    // At least one matched endpoint is rendered.
                    return None;
                }
                for filter in filters {
                    if !removed_by.contains(&filter) {
                        removed_by.push(filter);
                    }
                }
            }
            Some(format!(
                "vimanam: {label} matched an operation removed by {}; nothing is rendered for it.",
                removed_by.join(", ")
            ))
        })
        .collect()
}

/// The candidate closest to `wanted` by edit distance, when it is close enough
/// to be a plausible typo (at most a third of `wanted`'s length, minimum 2).
/// Ties go to the first candidate in spec order, so the hint is deterministic.
fn suggest(wanted: &str, candidates: impl Iterator<Item = String>) -> Option<String> {
    let limit = (wanted.chars().count() / 3).max(2);
    let mut best: Option<(usize, String)> = None;
    for candidate in candidates {
        let distance = levenshtein(wanted, &candidate);
        if distance <= limit && best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Edit distance between two strings, counted in chars.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == *cb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_counts_edits() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("GET /user", "GET /users"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn suggest_prefers_closest_then_spec_order() {
        let candidates = || {
            ["GET /users", "GET /usurs", "DELETE /users/{id}"]
                .into_iter()
                .map(String::from)
        };
        // Both first candidates are one edit away; the first in order wins.
        assert_eq!(
            suggest("GET /user", candidates()).as_deref(),
            Some("GET /users")
        );
        assert_eq!(suggest("POST /completely/else", candidates()), None);
    }
}
