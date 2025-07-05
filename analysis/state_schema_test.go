package analysis

import (
	"testing"

	"github.com/j-clemons/dbt-language-server/lsp"
)

func TestGoToSchema(t *testing.T) {
	state := NewState()

	// Setup test data - simulate a dbt project with models and schema
	state.DbtContext.ModelDetailMap = map[string]ModelDetails{
		"customers": {
			URI:         "/test/models/customers.sql",
			ProjectName: "test_project",
			Description: "Customer model",
			SchemaURI:   "/test/models/schema.yml",
			SchemaRange: lsp.Range{
				Start: lsp.Position{Line: 5, Character: 10},
				End:   lsp.Position{Line: 5, Character: 10},
			},
		},
		"orders": {
			URI:         "/test/models/orders.sql",
			ProjectName: "test_project",
			Description: "Orders model",
			SchemaURI:   "/test/models/schema.yml",
			SchemaRange: lsp.Range{
				Start: lsp.Position{Line: 15, Character: 10},
				End:   lsp.Position{Line: 15, Character: 10},
			},
		},
	}

	// Test document with REF token
	testSQL := `
SELECT 
    customer_id,
    COUNT(*) as order_count
FROM {{ ref('customers') }}
GROUP BY customer_id
`

	state.parseDocument("file:///test/models/orders.sql", testSQL)

	tests := []struct {
		name             string
		uri              string
		position         lsp.Position
		expectedURI      string
		expectedLine     int
		shouldHaveResult bool
	}{
		{
			name:             "cursor on ref token should navigate to referenced model schema",
			uri:              "file:///test/models/orders.sql",
			position:         lsp.Position{Line: 4, Character: 15}, // Position on 'customers' in ref
			expectedURI:      "file:///test/models/schema.yml",
			expectedLine:     5,
			shouldHaveResult: true,
		},
		{
			name:             "cursor not on ref token should navigate to current file schema",
			uri:              "file:///test/models/orders.sql",
			position:         lsp.Position{Line: 1, Character: 0}, // Position on SELECT
			expectedURI:      "file:///test/models/schema.yml",
			expectedLine:     15,
			shouldHaveResult: true,
		},
		{
			name:             "model without schema should return nil result",
			uri:              "file:///test/models/nonexistent.sql",
			position:         lsp.Position{Line: 0, Character: 0},
			shouldHaveResult: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			response := state.GoToSchema(1, tt.uri, tt.position)

			if tt.shouldHaveResult {
				if response.Result == nil {
					t.Errorf("Expected result but got nil")
					return
				}

				location, ok := response.Result.(lsp.Location)
				if !ok {
					t.Errorf("Expected result to be lsp.Location, got %T", response.Result)
					return
				}

				if location.URI != tt.expectedURI {
					t.Errorf("Expected URI %s, got %s", tt.expectedURI, location.URI)
				}

				if location.Range.Start.Line != tt.expectedLine {
					t.Errorf("Expected line %d, got %d", tt.expectedLine, location.Range.Start.Line)
				}
			} else {
				if response.Result != nil {
					t.Errorf("Expected nil result but got %v", response.Result)
				}
			}
		})
	}
}

func TestGetModelNameFromURI(t *testing.T) {
	tests := []struct {
		name     string
		uri      string
		expected string
	}{
		{
			name:     "file URI with .sql extension",
			uri:      "file:///test/models/customers.sql",
			expected: "customers",
		},
		{
			name:     "file URI without file:// prefix",
			uri:      "/test/models/orders.sql",
			expected: "orders",
		},
		{
			name:     "file without .sql extension",
			uri:      "file:///test/models/schema.yml",
			expected: "",
		},
		{
			name:     "empty URI",
			uri:      "",
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := getModelNameFromURI(tt.uri)
			if result != tt.expected {
				t.Errorf("Expected %s, got %s", tt.expected, result)
			}
		})
	}
}
