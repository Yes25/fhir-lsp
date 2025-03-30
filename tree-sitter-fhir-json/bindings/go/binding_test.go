package tree_sitter_fhir_json_test

import (
	"testing"

	tree_sitter "github.com/tree-sitter/go-tree-sitter"
	tree_sitter_fhir_json "github.com/tree-sitter/tree-sitter-fhir_json/bindings/go"
)

func TestCanLoadGrammar(t *testing.T) {
	language := tree_sitter.NewLanguage(tree_sitter_fhir_json.Language())
	if language == nil {
		t.Errorf("Error loading FhirJson grammar")
	}
}
