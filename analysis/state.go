package analysis

import (
	"fmt"
	"regexp"
	"strings"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/util"
)

type State struct {
    Documents map[string]string
    DbtContext DbtContext
}

type DbtContext struct {
    ProjectRoot    string
    ProjectYaml    DbtProjectYaml
    ModelDetailMap map[string]ModelDetails
    MacroDetailMap map[string]Macro
}

func NewState() State {
    return State{
        Documents: map[string]string{},
        DbtContext: DbtContext{
            ProjectRoot: GetProjectRoot("dbt_project.yml"),
            ProjectYaml: ParseDbtProjectYaml(GetProjectRoot("dbt_project.yml")),
            ModelDetailMap: map[string]ModelDetails{},
            MacroDetailMap: map[string]Macro{},
        },
    }
}

func (s *State) refreshDbtContext() {
    s.DbtContext.ProjectRoot = GetProjectRoot("dbt_project.yml")

    s.DbtContext.ProjectYaml = ParseDbtProjectYaml(s.DbtContext.ProjectRoot)
    newModelDetailMap := GetModelDetails(s.DbtContext.ProjectRoot)
    for k, v := range newModelDetailMap {
        s.DbtContext.ModelDetailMap[k] = v
    }

    newMacroDetailMap := GetMacroDetails(s.DbtContext.ProjectRoot)
    for k, v := range newMacroDetailMap {
        s.DbtContext.MacroDetailMap[k] = v
    }
}

func (s *State) OpenDocument(uri, text string) {
    s.Documents[uri] = text
    s.refreshDbtContext()
}

func (s *State) UpdateDocument(uri, text string) {
    s.Documents[uri] = text
}

func (s *State) SaveDocument(uri string) {
    s.refreshDbtContext()
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

    cursorStr := util.StringUnderCursor(uri, position.Line, position.Character)

    if s.DbtContext.ModelDetailMap[cursorStr].URI != "" {
        response.Result.URI = "file://" + s.DbtContext.ModelDetailMap[cursorStr].URI
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
    } else if s.DbtContext.MacroDetailMap[cursorStr].URI != "" {
        response.Result.URI = "file://" + s.DbtContext.MacroDetailMap[cursorStr].URI
        response.Result.Range = s.DbtContext.MacroDetailMap[cursorStr].Range
    }

	return response
}

func (s *State) TextDocumentCompletion(id int, uri string, position lsp.Position) lsp.CompletionResponse {
    items := []lsp.CompletionItem{}

    fileContents := s.Documents[uri]
    lines := strings.Split(fileContents, "\n")
    lineText := lines[position.Line]

    cursorOffset := int(position.Character)
    textBeforeCursor := lineText[:cursorOffset]

    refRegex := regexp.MustCompile(`\bref\(('|")[a-zA-z]*$`)
    jinjaBlockRegex := regexp.MustCompile(`\{\{\s*`)

    if refRegex.MatchString(textBeforeCursor) {
        items = GetRefCompletionItems(
            s.DbtContext.ModelDetailMap,
            GetReferenceSuffix(textBeforeCursor),
        )
    } else if jinjaBlockRegex.MatchString(textBeforeCursor) {
        items = GetMacroCompletionItems(s.DbtContext.MacroDetailMap, s.DbtContext.ProjectYaml)
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
