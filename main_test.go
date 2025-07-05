package main

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	"github.com/j-clemons/dbt-language-server/analysis"
	"github.com/j-clemons/dbt-language-server/analysis/parser"
	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/rpc"
)

func TestExecuteCommandHandling(t *testing.T) {
	// Create a test state with sample data
	state := analysis.NewState()
	state.DbtContext.ModelDetailMap = map[string]analysis.ModelDetails{
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
	}

	// Test document with REF token
	testSQL := `SELECT * FROM {{ ref('customers') }}`
	state.Documents = map[string]analysis.Document{}

	// Manually create document since parseDocument is unexported
	parserIns := parser.Parse(testSQL, "duckdb")
	state.Documents["file:///test/models/orders.sql"] = analysis.Document{
		Text:      testSQL,
		Tokens:    parserIns.CreateTokenIndex(),
		DefTokens: parserIns.CreateTokenNameMap(),
	}

	// Create test execute command request
	request := lsp.ExecuteCommandRequest{
		Request: lsp.Request{
			RPC:    "2.0",
			ID:     1,
			Method: "workspace/executeCommand",
		},
		Params: lsp.ExecuteCommandParams{
			Command: "dbt.goToSchema",
			Arguments: []any{
				map[string]any{
					"uri": "file:///test/models/orders.sql",
					"position": map[string]any{
						"line":      float64(0),
						"character": float64(25), // Position on 'customers' in ref
					},
				},
			},
		},
	}

	// Marshal to JSON to simulate real request
	requestBytes, err := json.Marshal(request)
	if err != nil {
		t.Fatalf("Failed to marshal request: %v", err)
	}

	// Test the JSON parsing logic from main.go
	var parsedRequest lsp.ExecuteCommandRequest
	if err := json.Unmarshal(requestBytes, &parsedRequest); err != nil {
		t.Fatalf("Failed to unmarshal request: %v", err)
	}

	// Verify the command is correct
	if parsedRequest.Params.Command != "dbt.goToSchema" {
		t.Errorf("Expected command 'dbt.goToSchema', got '%s'", parsedRequest.Params.Command)
	}

	// Test the argument parsing logic from main.go
	if len(parsedRequest.Params.Arguments) < 1 {
		t.Fatal("Expected at least 1 argument")
	}

	argMap, ok := parsedRequest.Params.Arguments[0].(map[string]interface{})
	if !ok {
		t.Fatalf("Expected first argument to be map[string]interface{}, got %T", parsedRequest.Params.Arguments[0])
	}

	uri, ok := argMap["uri"].(string)
	if !ok {
		t.Fatalf("Expected uri to be string, got %T", argMap["uri"])
	}

	positionMap, ok := argMap["position"].(map[string]interface{})
	if !ok {
		t.Fatalf("Expected position to be map[string]interface{}, got %T", argMap["position"])
	}

	line, ok := positionMap["line"].(float64)
	if !ok {
		t.Fatalf("Expected line to be float64, got %T", positionMap["line"])
	}

	character, ok := positionMap["character"].(float64)
	if !ok {
		t.Fatalf("Expected character to be float64, got %T", positionMap["character"])
	}

	// Create position and call GoToSchema
	position := lsp.Position{
		Line:      int(line),
		Character: int(character),
	}

	response := state.GoToSchema(parsedRequest.ID, uri, position)

	// Verify response
	if response.Result == nil {
		t.Fatal("Expected result but got nil")
	}

	location, ok := response.Result.(lsp.Location)
	if !ok {
		t.Fatalf("Expected result to be lsp.Location, got %T", response.Result)
	}

	expectedURI := "file:///test/models/schema.yml"
	if location.URI != expectedURI {
		t.Errorf("Expected URI %s, got %s", expectedURI, location.URI)
	}

	if location.Range.Start.Line != 5 {
		t.Errorf("Expected line 5, got %d", location.Range.Start.Line)
	}
}

func TestRPCMessageHandling(t *testing.T) {
	// Test the full RPC message flow
	executeCommandJSON := `{
		"jsonrpc": "2.0",
		"id": 1,
		"method": "workspace/executeCommand",
		"params": {
			"command": "dbt.goToSchema",
			"arguments": [
				{
					"uri": "file:///test/models/orders.sql",
					"position": {
						"line": 0,
						"character": 20
					}
				}
			]
		}
	}`

	// Test RPC message decoding
	rpcMessage := fmt.Sprintf("Content-Length: %d\r\n\r\n%s", len(executeCommandJSON), executeCommandJSON)
	method, contents, err := rpc.DecodeMessage([]byte(rpcMessage))
	if err != nil {
		t.Fatalf("Failed to decode RPC message: %v", err)
	}

	if method != "workspace/executeCommand" {
		t.Errorf("Expected method 'workspace/executeCommand', got '%s'", method)
	}

	// Test JSON unmarshaling
	var request lsp.ExecuteCommandRequest
	if err := json.Unmarshal(contents, &request); err != nil {
		t.Fatalf("Failed to unmarshal execute command request: %v", err)
	}

	if request.Params.Command != "dbt.goToSchema" {
		t.Errorf("Expected command 'dbt.goToSchema', got '%s'", request.Params.Command)
	}
}

func TestResponseEncoding(t *testing.T) {
	// Test that responses are properly encoded
	response := lsp.ExecuteCommandResponse{
		Response: lsp.Response{
			RPC: "2.0",
			ID:  func() *int { i := 1; return &i }(),
		},
		Result: lsp.Location{
			URI: "file:///test/schema.yml",
			Range: lsp.Range{
				Start: lsp.Position{Line: 5, Character: 10},
				End:   lsp.Position{Line: 5, Character: 10},
			},
		},
	}

	// Test encoding
	encoded := rpc.EncodeMessage(response)

	// Should contain proper headers and JSON
	if !strings.Contains(encoded, "Content-Length:") {
		t.Error("Expected Content-Length header in encoded message")
	}

	if !strings.Contains(encoded, `"jsonrpc":"2.0"`) {
		t.Error("Expected jsonrpc field in encoded message")
	}

	if !strings.Contains(encoded, `"file:///test/schema.yml"`) {
		t.Error("Expected URI in encoded message")
	}
}
