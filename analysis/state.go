package analysis

import (
	"fmt"

	"github.com/j-clemons/dbt-language-server/lsp"
)

type State struct {
    Documents map[string]string
}

func NewState() State {
    return State{Documents: map[string]string{}}
}

func (s *State) OpenDocument(uri, text string) {
    s.Documents[uri] = text
}

func (s *State) UpdateDocument(uri, text string) {
    s.Documents[uri] = text
}

func (s *State) Hover(id int, uri string, position lsp.Position) lsp.HoverResponse {
    // this should look up the type in the type analysis code
    document := s.Documents[uri]

    return lsp.HoverResponse{
        Response: lsp.Response{
            RPC: "2.0",
            ID:  &id,
        },
        Result: lsp.HoverResult{
            Contents: fmt.Sprintf("File: %s, Characters: %d", uri, len(document)),
        },
    }
}

func (s *State) Definition(id int, uri string, position lsp.Position) lsp.DefinitionResponse {
// In real life, this would look up the definition

	return lsp.DefinitionResponse{
		Response: lsp.Response{
			RPC: "2.0",
			ID:  &id,
		},
		Result: lsp.Location{
			URI: uri,
			Range: lsp.Range{
				Start: lsp.Position{
					Line:      position.Line - 1,
					Character: 0,
				},
				End: lsp.Position{
					Line:      position.Line - 1,
					Character: 0,
				},
			},
		},
	}
}
