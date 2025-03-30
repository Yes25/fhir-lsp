import XCTest
import SwiftTreeSitter
import TreeSitterFhirJson

final class TreeSitterFhirJsonTests: XCTestCase {
    func testCanLoadGrammar() throws {
        let parser = Parser()
        let language = Language(language: tree_sitter_fhir_json())
        XCTAssertNoThrow(try parser.setLanguage(language),
                         "Error loading FhirJson grammar")
    }
}
