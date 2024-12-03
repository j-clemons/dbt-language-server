package analysis

import (
	"fmt"
	"regexp"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/util"
)

type State struct {
    Documents map[string]string
    DbtContext DbtContext
}

type DbtContext struct {
    ModelPathMap map[string]string
}

func NewState() State {
    return State{
        Documents: map[string]string{},
        DbtContext: DbtContext{
            ModelPathMap: util.CreateModelPathMap(),
        },
    }
}

func (s *State) OpenDocument(uri, text string) {
    s.Documents[uri] = text
}

func (s *State) UpdateDocument(uri, text string) {
    s.Documents[uri] = text
}

func (s *State) SaveDocument(uri string) {
    newModelPathMap := util.CreateModelPathMap()
    for k, v := range newModelPathMap {
        s.DbtContext.ModelPathMap[k] = v
    }
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
    response := lsp.DefinitionResponse{
		Response: lsp.Response{
			RPC: "2.0",
			ID:  &id,
		},
		Result: lsp.Location{
            URI: uri,
            Range: lsp.Range{
                Start: lsp.Position{
                    Line:      position.Line,
                    Character: position.Character,
                },
                End: lsp.Position{
                    Line:      position.Line,
                    Character: position.Character,
                },
            },
        },
	}

    ref := util.GetRef(uri, position.Line, position.Character)

    if s.DbtContext.ModelPathMap[ref] != "" {
        response.Result.URI = "file://" + s.DbtContext.ModelPathMap[ref]
        response.Result.Range = lsp.Range{
                Start: lsp.Position{
                    Line:      0,
                    Character: 0,
                },
                End: lsp.Position{
                    Line:      0,
                    Character: 0,
                },
        }
    }

	return response
}

func (s *State) TextDocumentCompletion(id int, uri string, position lsp.Position) lsp.CompletionResponse {
    items := []lsp.CompletionItem{}

    fileContents := s.Documents[uri]
    lines := util.SplitContents(fileContents)
    lineText := lines[position.Line]

    cursorOffset := int(position.Character)
    textBeforeCursor := lineText[:cursorOffset]

    refRegex := regexp.MustCompile(`\bref\(('|")[a-zA-z]*$`)

    if refRegex.MatchString(textBeforeCursor) {
        items = GetRefCompletionItems(
            s.DbtContext.ModelPathMap,
            GetReferenceSuffix(textBeforeCursor),
        )
    }

    response := lsp.CompletionResponse{
        Response: lsp.Response{
            RPC: "2.0",
            ID:  &id,
        },
        Result: items,
    }

    return response
}
