use std::collections::HashMap;
use tower_lsp_server::ls_types::{Position, Range};
use tree_sitter::{Node, Tree};

/// Holds the extracted FHIR relationships for a single document.
///
/// Definitions map a canonical key ("ResourceType/id") to the range of that
/// resource's `id` value — where the cursor should land on goto-definition.
///
/// References map the same key to every range where a `"reference"` field
/// points to it — the locations returned by goto-references.
#[derive(Debug, Default)]
pub struct FhirIndex {
    pub definitions: HashMap<String, Range>,
    pub references: HashMap<String, Vec<Range>>,
}

impl FhirIndex {
    pub fn build(tree: &Tree, source: &str) -> Self {
        let mut index = FhirIndex::default();
        walk(tree.root_node(), source, &mut index);
        index
    }

    /// If `node` (or one of its ancestors up to the nearest `pair`) is inside
    /// the value of a `"reference"` field, returns the reference target string.
    pub fn reference_at(&self, node: Node, source: &str) -> Option<String> {
        find_reference_target(node, source)
    }

    /// Returns the "ResourceType/id" key for the nearest ancestor `object`
    /// that has both a `resourceType` and an `id` field.
    pub fn definition_key_at(&self, node: Node, source: &str) -> Option<String> {
        find_resource_key(node, source)
    }
}

// ── tree-sitter helpers ──────────────────────────────────────────────────────

fn to_lsp_range(node: Node) -> Range {
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

/// Returns the text inside a `string` node (the content between the quotes).
fn string_content<'a>(string_node: Node<'a>, source: &'a str) -> Option<&'a str> {
    let mut cursor = string_node.walk();
    for child in string_node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return child.utf8_text(source.as_bytes()).ok();
        }
    }
    None
}

/// Returns the text of the key of a `pair` node.
fn pair_key<'a>(pair: Node<'a>, source: &'a str) -> Option<&'a str> {
    string_content(pair.child_by_field_name("key")?, source)
}

// ── index building ───────────────────────────────────────────────────────────

fn walk(node: Node, source: &str, index: &mut FhirIndex) {
    if node.kind() == "object" {
        scan_object(node, source, index);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, index);
    }
}

/// Scans the direct `pair` children of an object node.
///
/// - Records a definition if the object has both `resourceType` and `id`.
/// - Records a reference for every `"reference"` field found (these may appear
///   in any object, not just resource roots).
fn scan_object(object: Node, source: &str, index: &mut FhirIndex) {
    let mut resource_type: Option<String> = None;
    let mut id: Option<String> = None;
    let mut id_range: Option<Range> = None;

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

        match key {
            "resourceType" => {
                resource_type = string_content(value, source).map(str::to_owned);
            }
            "id" => {
                if let Some(v) = string_content(value, source) {
                    id = Some(v.to_owned());
                    id_range = Some(to_lsp_range(value));
                }
            }
            "reference" => {
                if let Some(v) = string_content(value, source) {
                    index
                        .references
                        .entry(v.to_owned())
                        .or_default()
                        .push(to_lsp_range(value));
                }
            }
            _ => {}
        }
    }

    if let (Some(rt), Some(id_val), Some(range)) = (resource_type, id, id_range) {
        index
            .definitions
            .insert(format!("{}/{}", rt, id_val), range);
    }
}

// ── cursor-based lookup ──────────────────────────────────────────────────────

/// Walks up from `node` to the nearest enclosing `pair`. If that pair's key
/// is `"reference"`, returns its string value; otherwise returns `None`.
fn find_reference_target(node: Node, source: &str) -> Option<String> {
    let mut current = node;
    loop {
        if current.kind() == "pair" {
            if pair_key(current, source)? == "reference" {
                let value = current.child_by_field_name("value")?;
                return string_content(value, source).map(str::to_owned);
            }
            return None;
        }
        current = current.parent()?;
    }
}

/// Walks up from `node` through ancestor `object` nodes until one is found
/// that has both `resourceType` and `id` direct children, then returns the
/// canonical key "ResourceType/id".
fn find_resource_key(node: Node, source: &str) -> Option<String> {
    let mut current = node;
    loop {
        if current.kind() == "object" {
            let mut resource_type: Option<String> = None;
            let mut id: Option<String> = None;

            let mut cursor = current.walk();
            for pair in current.children(&mut cursor) {
                if pair.kind() != "pair" {
                    continue;
                }
                let Some(key) = pair_key(pair, source) else {
                    continue;
                };
                let Some(value) = pair.child_by_field_name("value") else {
                    continue;
                };
                match key {
                    "resourceType" => {
                        resource_type = string_content(value, source).map(str::to_owned);
                    }
                    "id" => {
                        id = string_content(value, source).map(str::to_owned);
                    }
                    _ => {}
                }
                if resource_type.is_some() && id.is_some() {
                    break;
                }
            }

            if let (Some(rt), Some(id_val)) = (resource_type, id) {
                return Some(format!("{}/{}", rt, id_val));
            }
        }
        current = current.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::{Parser, Point};

    fn fixture() -> (String, tree_sitter::Tree, FhirIndex) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tree-sitter-fhir-json/test_files/test_bundle.json");
        let source = std::fs::read_to_string(path).unwrap();

        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_fhir_json::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&source, None).unwrap();
        let index = FhirIndex::build(&tree, &source);

        (source, tree, index)
    }

    fn node_at(tree: &'_ tree_sitter::Tree, row: usize, col: usize) -> tree_sitter::Node<'_> {
        let point = Point { row, column: col };
        tree.root_node()
            .descendant_for_point_range(point, point)
            .unwrap()
    }

    // ── index contents ───────────────────────────────────────────────────────

    #[test]
    fn indexes_all_resource_definitions() {
        let (_, _, index) = fixture();
        assert!(index.definitions.contains_key("Bundle/bundle-example"));
        assert!(index.definitions.contains_key("MedicationRequest/3123"));
        assert!(index.definitions.contains_key("Medication/example"));
        assert_eq!(index.definitions.len(), 3, "unexpected extra definitions");
    }

    #[test]
    fn indexes_outbound_references() {
        let (_, _, index) = fixture();
        assert_eq!(
            index.references.get("Medication/example").map(Vec::len),
            Some(1),
        );
        assert_eq!(index.references.get("Patient/347").map(Vec::len), Some(1),);
        assert_eq!(index.references.len(), 2, "unexpected extra reference keys");
    }

    #[test]
    fn definition_ranges_point_to_id_value() {
        let (_, _, index) = fixture();

        // file line 21 (row 20): `      "id" : "3123",`
        let mr = index.definitions["MedicationRequest/3123"];
        assert_eq!(mr.start.line, 20);

        // file line 46 (row 45): `      "id" : "example",`
        let med = index.definitions["Medication/example"];
        assert_eq!(med.start.line, 45);
    }

    // ── cursor-based lookup ──────────────────────────────────────────────────

    #[test]
    fn reference_at_returns_target_string() {
        let (source, tree, index) = fixture();

        // row 29: `          "reference" : "Medication/example"`
        let node = node_at(&tree, 29, 28);
        assert_eq!(
            index.reference_at(node, &source),
            Some("Medication/example".to_owned()),
        );

        // row 33: `        "reference" : "Patient/347"`
        let node = node_at(&tree, 33, 26);
        assert_eq!(
            index.reference_at(node, &source),
            Some("Patient/347".to_owned()),
        );
    }

    #[test]
    fn reference_at_returns_none_for_non_reference_field() {
        let (source, tree, index) = fixture();

        // row 20: `      "id" : "3123"` — id field, not a reference
        let node = node_at(&tree, 20, 14);
        assert_eq!(index.reference_at(node, &source), None);
    }

    #[test]
    fn definition_key_at_finds_enclosing_resource() {
        let (source, tree, index) = fixture();

        // Cursor on the "3123" id value — inside MedicationRequest
        // row 20: `      "id" : "3123",`
        let node = node_at(&tree, 20, 14);
        assert_eq!(
            index.definition_key_at(node, &source),
            Some("MedicationRequest/3123".to_owned()),
        );

        // Cursor on the "Medication" resourceType value — inside Medication resource
        // row 44: `      "resourceType" : "Medication",`
        let node = node_at(&tree, 44, 25);
        assert_eq!(
            index.definition_key_at(node, &source),
            Some("Medication/example".to_owned()),
        );
    }

    #[test]
    fn goto_definition_resolves_reference_to_definition_range() {
        let (source, tree, index) = fixture();

        // Cursor on "Medication/example" reference value (row 29)
        let node = node_at(&tree, 29, 28);
        let ref_key = index.reference_at(node, &source).unwrap();
        let def_range = index.definitions.get(&ref_key).unwrap();

        // The definition should land on the Medication "id" value line
        assert_eq!(def_range.start.line, 45);
    }

    #[test]
    fn goto_references_from_resource_returns_all_reference_sites() {
        let (source, tree, index) = fixture();

        // Cursor on the Medication resource's id value (row 45)
        // row 45: `      "id" : "example",`
        let node = node_at(&tree, 45, 14);
        let key = index.definition_key_at(node, &source).unwrap();

        let refs = index.references.get(&key).unwrap();
        assert_eq!(refs.len(), 1);
        // The reference site is on row 29
        assert_eq!(refs[0].start.line, 29);
    }
}
