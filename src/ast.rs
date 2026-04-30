use tower_lsp_server::ls_types::{Position, Range};
use tree_sitter::Node;

// ── Node → LSP conversion ────────────────────────────────────────────────────

/// Converts a tree-sitter node's span to an LSP [`Range`].
pub fn to_lsp_range(node: Node) -> Range {
    Range {
        start: Position {
            line: node.start_position().row as u32,
            character: node.start_position().column as u32,
        },
        end: Position {
            line: node.end_position().row as u32,
            character: node.end_position().column as u32,
        },
    }
}

// ── String extraction ────────────────────────────────────────────────────────

/// Returns the text inside a `string` node — the content between the quotes,
/// with escape sequences left as-is.
pub fn string_content<'a>(string_node: Node<'a>, source: &'a str) -> Option<&'a str> {
    let mut cursor = string_node.walk();
    for child in string_node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return child.utf8_text(source.as_bytes()).ok();
        }
    }
    None
}

/// Returns the unquoted text of the key of a `pair` node.
pub fn pair_key<'a>(pair: Node<'a>, source: &'a str) -> Option<&'a str> {
    string_content(pair.child_by_field_name("key")?, source)
}

// ── Upward traversal ─────────────────────────────────────────────────────────

/// Walks up from `node` to the nearest enclosing `pair`.
///
/// Works regardless of whether the cursor is on a key, a value, or anywhere
/// inside a string (e.g. on a `string_content` leaf).
pub fn enclosing_pair(mut node: Node) -> Option<Node> {
    loop {
        if node.kind() == "pair" {
            return Some(node);
        }
        node = node.parent()?;
    }
}

/// Returns the value of the `"resourceType"` field that is a *direct* child
/// pair of `object`, or `None` if the object has no such field.
pub fn object_resource_type<'a>(object: Node<'a>, source: &'a str) -> Option<&'a str> {
    let mut cursor = object.walk();
    for child in object.children(&mut cursor) {
        if child.kind() != "pair" {
            continue;
        }
        if pair_key(child, source) == Some("resourceType") {
            let value = child.child_by_field_name("value")?;
            return string_content(value, source);
        }
    }
    None
}

/// Returns the `pair` node that directly or indirectly contains `node`.
///
/// Two cases arise in the JSON grammar:
/// - The node is the value of a pair directly → return that pair.
/// - The node is an element inside an array that is itself a pair's value →
///   return the pair that owns the array.
///
/// Returns `None` when `node` is at the document root with no enclosing pair.
pub fn pair_containing_node(node: Node) -> Option<Node> {
    let parent = node.parent()?;
    match parent.kind() {
        "pair" => Some(parent),
        "array" => {
            let grandparent = parent.parent()?;
            if grandparent.kind() == "pair" {
                Some(grandparent)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ── FHIR path resolution ─────────────────────────────────────────────────────

/// Resolves the FHIR element path for whatever field the cursor sits on.
///
/// Works whether the cursor is on a key, a value, or anywhere inside a string.
/// Array nesting is transparent — FHIR paths do not include array indices.
///
/// # Examples
/// ```text
/// cursor on "type" in Bundle root               → "Bundle.type"
/// cursor on "relation" in Bundle.link[]         → "Bundle.link.relation"
/// cursor on "status" in MedicationRequest.text  → "MedicationRequest.text.status"
/// cursor on inner "reference" in medication     → "MedicationRequest.medication.reference.reference"
/// ```
///
/// Returns `None` when the cursor is not inside a recognisable FHIR resource
/// (e.g. on the root brace of a non-resource document).
pub fn fhir_path_at(node: Node, source: &str) -> Option<String> {
    let start_pair = enclosing_pair(node)?;
    let leaf_key = pair_key(start_pair, source)?;

    let mut segments: Vec<String> = vec![leaf_key.to_owned()];
    let mut current = start_pair;

    loop {
        // A pair's parent in the grammar is always the containing object.
        let obj = current.parent()?;
        debug_assert_eq!(obj.kind(), "object");

        if let Some(rt) = object_resource_type(obj, source) {
            segments.reverse();
            return Some(format!("{}.{}", rt, segments.join(".")));
        }

        // Not a resource root — find which pair contains this object.
        let containing = pair_containing_node(obj)?;
        let key = pair_key(containing, source)?;
        segments.push(key.to_owned());
        current = containing;
    }
}
