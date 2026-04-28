use std::collections::HashMap;

use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, Position, Range};
use tree_sitter::{Node, Tree};

use crate::ast::{object_resource_type, pair_key, to_lsp_range};
use crate::definitions::{child_lookup_prefix, find_choice_type, ElementInfo};

/// Validates a parsed FHIR JSON document and returns a list of LSP diagnostics.
///
/// Syntax errors are checked first. If any are found, FHIR validation is
/// skipped — the AST is too incomplete to reason about structure reliably.
///
/// When the document is syntactically valid, three classes of issues are reported:
/// - Wrong JSON representation: an array where a scalar is expected, or vice-versa.
/// - Cardinality violations: more values than `max` allows.
/// - Missing required fields: a field with `min >= 1` is absent.
///
/// All checks apply recursively to backbone elements and named complex types
/// (e.g. `HumanName`, `Period`) within every resource found in the document.
pub fn validate(
    tree: &Tree,
    source: &str,
    defs: &HashMap<String, ElementInfo>,
) -> Vec<Diagnostic> {
    let mut diags = syntax_errors(tree, source);
    if diags.is_empty() {
        walk(tree.root_node(), source, defs, &mut diags);
    }
    diags
}

// ── Syntax error detection ────────────────────────────────────────────────────

fn syntax_errors(tree: &Tree, source: &str) -> Vec<Diagnostic> {
    if !tree.root_node().has_error() {
        return vec![];
    }
    let mut diags = Vec::new();
    collect_syntax_errors(tree.root_node(), source, &mut diags);
    diags
}

fn collect_syntax_errors(node: Node, source: &str, diags: &mut Vec<Diagnostic>) {
    if node.is_missing() {
        let msg = match node.kind() {
            "}" => "Missing `}` — unclosed object or missing comma before this point.",
            "]" => "Missing `]` — unclosed array.",
            "," => "Missing `,` — expected comma between elements.",
            ":" => "Missing `:` — expected colon after key.",
            other => &format!("Missing `{other}` in JSON."),
        };
        diags.push(make_diag(
            to_lsp_range(node),
            DiagnosticSeverity::ERROR,
            msg.to_string(),
        ));
        return;
    }
    if node.is_error() {
        let snippet = node
            .utf8_text(source.as_bytes())
            .unwrap_or("")
            .trim()
            .chars()
            .take(40)
            .collect::<String>();
        let msg = if snippet.is_empty() {
            "JSON syntax error.".to_string()
        } else {
            format!("JSON syntax error: unexpected `{snippet}`.")
        };
        diags.push(make_diag(to_lsp_range(node), DiagnosticSeverity::ERROR, msg));
        // Don't recurse — ERROR subtrees produce cascading noise.
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_syntax_errors(child, source, diags);
    }
}

// ── Outer AST walk: finds resource roots ─────────────────────────────────────

/// Walks the whole tree looking for objects that have a `resourceType` field.
/// Nested resources (e.g. inside a Bundle) are found by this same walk after
/// the outer resource is processed; `validate_object_at_path` skips them to
/// avoid double-validation.
fn walk(
    node: Node,
    source: &str,
    defs: &HashMap<String, ElementInfo>,
    diags: &mut Vec<Diagnostic>,
) {
    if node.kind() == "object" {
        if let Some(rt) = object_resource_type(node, source) {
            let anchor = resource_type_value_range(node, source);
            validate_object_at_path(node, rt, anchor, source, defs, diags);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, defs, diags);
    }
}

// ── Per-object validation ─────────────────────────────────────────────────────

struct FieldOccurrence {
    count: usize,
    is_array: bool,
    key_range: Range,
}

/// Validates the direct-child fields of `object` against the FHIR definitions
/// found under the `fhir_path` prefix, then recurses into structured children.
///
/// `fhir_path` is either a resource type name (`"Patient"`), a backbone element
/// path (`"MedicationRequest.dispenseRequest"`), or a complex type name
/// (`"HumanName"`, `"Period"`).
///
/// `anchor` is the range underlined when a required child field is absent.
fn validate_object_at_path<'tree>(
    object: Node<'tree>,
    fhir_path: &str,
    anchor: Range,
    source: &str,
    defs: &HashMap<String, ElementInfo>,
    diags: &mut Vec<Diagnostic>,
) {
    let mut present: HashMap<String, FieldOccurrence> = HashMap::new();
    // Store value nodes separately — Node<'tree> borrows from the tree lifetime
    // and can't be stored inside the HashMap alongside non-tree data.
    let mut value_nodes: Vec<(String, Node<'tree>, Range)> = Vec::new();

    let mut cursor = object.walk();
    for pair in object.children(&mut cursor) {
        if pair.kind() != "pair" {
            continue;
        }
        let Some(key) = pair_key(pair, source) else {
            continue;
        };
        let Some(value) = pair.child_by_field_name("value") else {
            continue;
        };

        let is_array = value.kind() == "array";
        let count = if is_array { value.named_child_count() } else { 1 };
        let key_range = pair
            .child_by_field_name("key")
            .map(to_lsp_range)
            .unwrap_or_else(|| to_lsp_range(pair));

        present.insert(key.to_owned(), FieldOccurrence { count, is_array, key_range });
        value_nodes.push((key.to_owned(), value, key_range));
    }

    check_present_fields(fhir_path, &present, defs, diags);
    check_missing_required(fhir_path, &present, defs, diags, anchor);

    for (field, value_node, key_range) in value_nodes {
        let field_path = format!("{fhir_path}.{field}");
        descend(value_node, &field_path, key_range, source, defs, diags);
    }
}

/// Decides whether and how to recurse into a field's value node.
///
/// - `BackboneElement`: children are stored in the defs under the full field
///   path (`MedicationRequest.dispenseRequest.*`), so pass it through unchanged.
/// - Named complex type (`HumanName`, `Period`, …): children are stored under
///   the type name (`HumanName.*`), so switch to the type name as prefix.
/// - Primitive or `Resource`: no structured child validation — stop here.
/// - Objects that carry their own `resourceType` are nested resources handled
///   by the outer `walk`; skip them to avoid double-validation.
fn descend<'tree>(
    value: Node<'tree>,
    field_path: &str,
    key_range: Range,
    source: &str,
    defs: &HashMap<String, ElementInfo>,
    diags: &mut Vec<Diagnostic>,
) {
    let Some(child_path) = child_validation_path(field_path, defs) else {
        return;
    };

    match value.kind() {
        "object" => {
            if object_resource_type(value, source).is_none() {
                validate_object_at_path(value, &child_path, key_range, source, defs, diags);
            }
        }
        "array" => {
            let mut cursor = value.walk();
            for elem in value.named_children(&mut cursor) {
                if elem.kind() == "object" && object_resource_type(elem, source).is_none() {
                    validate_object_at_path(elem, &child_path, key_range, source, defs, diags);
                }
            }
        }
        _ => {}
    }
}

/// Returns the FHIR path prefix to use when validating children of `field_path`.
///
/// Delegates entirely to [`child_lookup_prefix`], which handles regular fields,
/// backbone elements, and choice types uniformly and recursively.
fn child_validation_path(field_path: &str, defs: &HashMap<String, ElementInfo>) -> Option<String> {
    child_lookup_prefix(field_path, defs)
}

// ── Field-level checks ────────────────────────────────────────────────────────

fn check_present_fields(
    fhir_path: &str,
    present: &HashMap<String, FieldOccurrence>,
    defs: &HashMap<String, ElementInfo>,
    diags: &mut Vec<Diagnostic>,
) {
    for (field, occ) in present {
        let path = format!("{fhir_path}.{field}");
        // For choice types (e.g. "itemCodeableConcept"), fall back to the "[x]" entry.
        let Some(info) = defs
            .get(&path)
            .or_else(|| find_choice_type(fhir_path, field, defs).map(|(i, _)| i))
        else {
            continue;
        };

        let max_is_one = info.max == "1";
        let max_is_many = info.max == "*" || info.max.parse::<usize>().map_or(false, |n| n > 1);

        if occ.is_array && max_is_one {
            diags.push(make_diag(
                occ.key_range,
                DiagnosticSeverity::ERROR,
                format!(
                    "`{path}` has max cardinality 1 and must be a single value, not an array."
                ),
            ));
            continue;
        }
        if !occ.is_array && max_is_many {
            diags.push(make_diag(
                occ.key_range,
                DiagnosticSeverity::ERROR,
                format!(
                    "`{path}` has max cardinality `{}` and must be represented as a JSON array.",
                    info.max
                ),
            ));
            continue;
        }

        if !max_is_one && info.max != "*" {
            if let Ok(max) = info.max.parse::<usize>() {
                if occ.count > max {
                    diags.push(make_diag(
                        occ.key_range,
                        DiagnosticSeverity::ERROR,
                        format!(
                            "`{path}` allows at most {max} item(s), but {} {} found.",
                            occ.count,
                            if occ.count == 1 { "was" } else { "were" },
                        ),
                    ));
                }
            }
        }
    }
}

fn check_missing_required(
    fhir_path: &str,
    present: &HashMap<String, FieldOccurrence>,
    defs: &HashMap<String, ElementInfo>,
    diags: &mut Vec<Diagnostic>,
    anchor: Range,
) {
    let prefix = format!("{fhir_path}.");
    for (path, info) in defs {
        if info.min == 0 || !path.starts_with(&prefix) {
            continue;
        }
        let rest = &path[prefix.len()..];
        if rest.contains('.') {
            continue; // only direct children of this level
        }

        // FHIR choice types appear in JSON as e.g. "valueString" / "valueReference",
        // while the spec names them "value[x]". Any key with the base prefix counts.
        let is_present = if let Some(base) = rest.strip_suffix("[x]") {
            present.keys().any(|k| k.starts_with(base))
        } else {
            present.contains_key(rest)
        };

        if !is_present {
            diags.push(make_diag(
                anchor,
                DiagnosticSeverity::WARNING,
                format!("Required field `{path}` is missing (min cardinality: {}).", info.min),
            ));
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns the range of the `"resourceType"` value in `object`, or a
/// single-character range at the opening brace when no such field exists.
fn resource_type_value_range(object: Node, source: &str) -> Range {
    let mut cursor = object.walk();
    for pair in object.children(&mut cursor) {
        if pair.kind() != "pair" {
            continue;
        }
        if pair_key(pair, source) == Some("resourceType") {
            if let Some(value) = pair.child_by_field_name("value") {
                return to_lsp_range(value);
            }
        }
    }
    let start = Position {
        line: object.start_position().row as u32,
        character: object.start_position().column as u32,
    };
    Range {
        start,
        end: Position { character: start.character + 1, ..start },
    }
}

fn make_diag(range: Range, severity: DiagnosticSeverity, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(severity),
        message,
        source: Some("fhir-lsp".to_string()),
        ..Default::default()
    }
}
