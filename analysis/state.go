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
    VariableMap    map[string]interface{}
}

func NewState() State {
    return State{
        Documents: map[string]string{},
        DbtContext: DbtContext{
            ProjectRoot:    "",
            ProjectYaml:    DbtProjectYaml{},
            ModelDetailMap: map[string]ModelDetails{},
            MacroDetailMap: map[string]Macro{},
            VariableMap:    map[string]interface{}{},
        },
    }
}

func (s *State) refreshDbtContext() {
    s.DbtContext.ProjectRoot = util.GetProjectRoot("dbt_project.yml")

    s.DbtContext.ProjectYaml = parseDbtProjectYaml(s.DbtContext.ProjectRoot)
    newModelDetailMap := getModelDetails(s.DbtContext.ProjectRoot)
    for k, v := range newModelDetailMap {
        s.DbtContext.ModelDetailMap[k] = v
    }

    newMacroDetailMap := getMacroDetails(s.DbtContext.ProjectRoot)
    for k, v := range newMacroDetailMap {
        s.DbtContext.MacroDetailMap[k] = v
    }

    newVariableMap := getProjectVariables(s.DbtContext.ProjectYaml)
    for k, v := range newVariableMap {
        s.DbtContext.VariableMap[k] = v
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
    response := lsp.HoverResponse{
        Response: lsp.Response{
            RPC: "2.0",
            ID:  &id,
        },
        Result: lsp.HoverResult{
            Contents: "",
        },
    }

    cursorStr := util.GetStringUnderCursor(s.Documents[uri], position.Line, position.Character)

    if s.DbtContext.ModelDetailMap[cursorStr].URI != "" {
        response.Result.Contents = s.DbtContext.ModelDetailMap[cursorStr].Description
    } else if s.DbtContext.MacroDetailMap[cursorStr].URI != "" {
        response.Result.Contents = s.DbtContext.MacroDetailMap[cursorStr].Description
    } else if s.DbtContext.VariableMap[cursorStr] != nil {
        response.Result.Contents = fmt.Sprintf(
            "%v: %v",
            cursorStr,
            s.DbtContext.VariableMap[cursorStr],
        )
    }
    return response
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

    cursorStr := util.GetStringUnderCursor(s.Documents[uri], position.Line, position.Character)

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
    textAfterCursor := lineText[cursorOffset:]

    refRegex := regexp.MustCompile(`\bref\(('|")[a-zA-z]*$`)
    varRegex := regexp.MustCompile(`\bvar\(('|")[a-zA-z]*$`)
    jinjaBlockRegex := regexp.MustCompile(`\{\{\s*`)

    if refRegex.MatchString(textBeforeCursor) {
        items = getRefCompletionItems(
            s.DbtContext.ModelDetailMap,
            getReferenceSuffix(lineText, textAfterCursor),
        )
    } else if varRegex.MatchString(textBeforeCursor) {
        items = getVariableCompletionItems(
            s.DbtContext.VariableMap,
            getVariableSuffix(textBeforeCursor, textAfterCursor),
        )
    } else if jinjaBlockRegex.MatchString(textBeforeCursor) {
        items = getMacroCompletionItems(s.DbtContext.MacroDetailMap, s.DbtContext.ProjectYaml)
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
