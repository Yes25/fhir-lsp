use std::collections::HashMap;

use serde_json::{Map, Value};
use tower_lsp_server::ls_types::{Position, Range, TextEdit};

use crate::definitions::{child_lookup_prefix, find_choice_type, ElementInfo};

/// Formats `source` as pretty-printed JSON with FHIR-spec field ordering.
///
/// Returns `None` if `source` is not valid JSON (the editor should not receive
/// an edit that would corrupt a document the user is actively fixing).
pub fn format_document(
    source: &str,
    insert_spaces: bool,
    tab_size: u32,
    defs: &HashMap<String, ElementInfo>,
) -> Option<Vec<TextEdit>> {
    let mut value = serde_json::from_str::<Value>(source).ok()?;

    reorder(&mut value, defs);

    let indent = if insert_spaces {
        " ".repeat(tab_size as usize)
    } else {
        "\t".to_string()
    };

    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&value, &mut ser).ok()?;
    let formatted = String::from_utf8(buf).ok()?;

    let newline_count = source.chars().filter(|&c| c == '\n').count() as u32;
    let last_line_len = source.rsplit('\n').next().unwrap_or("").chars().count() as u32;
    let end = Position { line: newline_count, character: last_line_len };

    Some(vec![TextEdit {
        range: Range { start: Position { line: 0, character: 0 }, end },
        new_text: formatted,
    }])
}

/// Recursively reorders every JSON object's keys to match the FHIR spec order.
///
/// `resourceType` is always placed first. Known FHIR fields follow in the order
/// they appear in the StructureDefinition snapshot. Unrecognised fields (e.g.
/// extensions, custom keys) are placed at the end in their original order.
pub fn reorder(value: &mut Value, defs: &HashMap<String, ElementInfo>) {
    reorder_value(value, "", defs);
}

fn reorder_value(value: &mut Value, fhir_path: &str, defs: &HashMap<String, ElementInfo>) {
    match value {
        Value::Object(map) => {
            let path = map
                .get("resourceType")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| fhir_path.to_owned());
            reorder_object(map, &path, defs);
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                reorder_value(item, fhir_path, defs);
            }
        }
        _ => {}
    }
}

fn reorder_object(map: &mut Map<String, Value>, fhir_path: &str, defs: &HashMap<String, ElementInfo>) {
    let pairs: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();

    let mut indexed: Vec<(SortKey, String, Value)> = pairs
        .into_iter()
        .map(|(k, v)| (sort_key(fhir_path, &k, defs), k, v))
        .collect();
    indexed.sort_by(|(a, ak, _), (b, bk, _)| a.cmp(b).then(ak.cmp(bk)));

    for (_, key, mut val) in indexed {
        let field_path = format!("{fhir_path}.{key}");
        if let Some(child_path) = child_lookup_prefix(&field_path, defs) {
            reorder_value(&mut val, &child_path, defs);
        }
        map.insert(key, val);
    }
}

/// Three-tier sort key:
/// - 0 = `resourceType` (always first)
/// - 1 = known FHIR field (ordered by spec position)
/// - 2 = unknown field (stable: preserves original relative order via large index)
#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct SortKey(u8, u32);

fn sort_key(fhir_path: &str, field: &str, defs: &HashMap<String, ElementInfo>) -> SortKey {
    if field == "resourceType" {
        return SortKey(0, 0);
    }
    let path = format!("{fhir_path}.{field}");
    let order = defs
        .get(&path)
        .or_else(|| find_choice_type(fhir_path, field, defs).map(|(i, _)| i))
        .map(|info| info.order);
    match order {
        Some(o) => SortKey(1, o),
        None => SortKey(2, 0),
    }
}
