use tree_sitter::{Parser, Language};

unsafe extern "C" { fn tree_sitter_fhir_json() -> Language; }

fn main() {
    println!("Hello, world!");
}

#[test]
fn test_parser() {
    let language = unsafe { tree_sitter_fhir_json() };
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();

    let source_code = "{\"hello\" : \"world}\"}";
    let tree = parser.parse(source_code, None).unwrap();

    assert_eq!(tree.root_node().to_sexp(), "(document (object (generic_key_val key: (string (string_content)) value: (string (string_content)))))");
}
