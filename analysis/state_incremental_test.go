package analysis

import (
	"testing"

	"github.com/j-clemons/dbt-language-server/lsp"
)

func TestUpdateDocumentIncremental(t *testing.T) {
	state := NewState()
	uri := "test://document.sql"
	initialText := "SELECT * FROM table1\nWHERE id = 1"

	state.OpenDocument(uri, initialText)

	if state.Documents[uri].Text != initialText {
		t.Errorf("Expected initial text to be %q, got %q", initialText, state.Documents[uri].Text)
	}

	changes := []lsp.TextDocumentContentChangeEvent{
		{
			Range: lsp.Range{
				Start: lsp.Position{Line: 1, Character: 11},
				End:   lsp.Position{Line: 1, Character: 12},
			},
			Text: "2",
		},
	}

	state.UpdateDocumentIncremental(uri, changes)

	expectedText := "SELECT * FROM table1\nWHERE id = 2"
	if state.Documents[uri].Text != expectedText {
		t.Errorf("Expected text after incremental update to be %q, got %q", expectedText, state.Documents[uri].Text)
	}
}

func TestApplyIncrementalChange(t *testing.T) {
	state := NewState()

	tests := []struct {
		name     string
		text     string
		change   lsp.TextDocumentContentChangeEvent
		expected string
	}{
		{
			name: "single character replacement",
			text: "hello world",
			change: lsp.TextDocumentContentChangeEvent{
				Range: lsp.Range{
					Start: lsp.Position{Line: 0, Character: 6},
					End:   lsp.Position{Line: 0, Character: 11},
				},
				Text: "Go",
			},
			expected: "hello Go",
		},
		{
			name: "multiline replacement",
			text: "line1\nline2\nline3",
			change: lsp.TextDocumentContentChangeEvent{
				Range: lsp.Range{
					Start: lsp.Position{Line: 0, Character: 4},
					End:   lsp.Position{Line: 1, Character: 4},
				},
				Text: "X\nnewline",
			},
			expected: "lineX\nnewline2\nline3",
		},
		{
			name: "insertion at beginning",
			text: "world",
			change: lsp.TextDocumentContentChangeEvent{
				Range: lsp.Range{
					Start: lsp.Position{Line: 0, Character: 0},
					End:   lsp.Position{Line: 0, Character: 0},
				},
				Text: "hello ",
			},
			expected: "hello world",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := state.applyIncrementalChange(tt.text, tt.change)
			if result != tt.expected {
				t.Errorf("Expected %q, got %q", tt.expected, result)
			}
		})
	}
}

func TestFullDocumentReplacement(t *testing.T) {
	state := NewState()
	uri := "test://document.sql"
	initialText := "SELECT * FROM table1"

	state.OpenDocument(uri, initialText)

	changes := []lsp.TextDocumentContentChangeEvent{
		{
			Range: lsp.Range{},
			Text:  "SELECT * FROM table2",
		},
	}

	state.UpdateDocumentIncremental(uri, changes)

	expectedText := "SELECT * FROM table2"
	if state.Documents[uri].Text != expectedText {
		t.Errorf("Expected text after full replacement to be %q, got %q", expectedText, state.Documents[uri].Text)
	}
}
