package analysis

import (
	"fmt"
	"regexp"
	"strings"

	"github.com/j-clemons/dbt-language-server/analysis/parser"
	"github.com/j-clemons/dbt-language-server/docs"
	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/util"
)

type State struct {
    Documents  map[string]Document
    DbtContext DbtContext
}

type Document struct {
    Text      string
    Tokens    *parser.TokenIndex
    DefTokens map[string]parser.Token
}

type DbtContext struct {
    ProjectRoot       string
    ProjectYaml       DbtProjectYaml
    Dialect           docs.Dialect
    ModelDetailMap    map[string]ModelDetails
    MacroDetailMap    map[Package]map[string]Macro
    VariableDetailMap map[string]Variable
}

func NewState() State {
    return State{
        Documents:  map[string]Document{},
        DbtContext: DbtContext{
            ProjectRoot:       "",
            ProjectYaml:       DbtProjectYaml{},
            Dialect:           "",
            ModelDetailMap:    map[string]ModelDetails{},
            MacroDetailMap:    map[Package]map[string]Macro{},
            VariableDetailMap: map[string]Variable{},
        },
    }
}

func (s *State) refreshDbtContext(wd string) {
    s.DbtContext.ProjectRoot = util.GetProjectRoot("dbt_project.yml", wd)

    s.DbtContext.ProjectYaml = parseDbtProjectYaml(s.DbtContext.ProjectRoot)
    s.DbtContext.Dialect = util.GetDialect(s.DbtContext.ProjectYaml.Profile.Value, wd)

    s.DbtContext.ModelDetailMap = getModelDetails(s.DbtContext.ProjectRoot)
    s.DbtContext.MacroDetailMap = getMacroDetails(s.DbtContext.ProjectRoot)
    s.DbtContext.VariableDetailMap = getProjectVariables(s.DbtContext.ProjectYaml, s.DbtContext.ProjectRoot)
}

func (s *State) parseDocument(uri, text string) {
    parserIns := parser.Parse(text, s.DbtContext.Dialect)
    s.Documents[uri] = Document{
        Text:      text,
        Tokens:    parserIns.CreateTokenIndex(),
        DefTokens: parserIns.CreateTokenNameMap(),
    }
}

func (s *State) OpenDocument(uri, text string) {
    s.refreshDbtContext("")
    s.parseDocument(uri, text)
}

func (s *State) UpdateDocument(uri, text string) {
    s.parseDocument(uri, text)
}

func (s *State) SaveDocument(uri string) {
    s.refreshDbtContext("")
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

    cursorTokenLL, err := s.Documents[uri].Tokens.FindTokenAtCursor(position.Line, position.Character)
    if err != nil {
        return response
    }

    cursorToken := cursorTokenLL.Token

    dialectFunctions := s.DbtContext.Dialect.FunctionDocs()

    switch cursorToken.Type {
    case parser.REF:
        response.Result.Contents = s.DbtContext.ModelDetailMap[cursorToken.Literal].Description
    case parser.VAR:
        response.Result.Contents = fmt.Sprintf(
            "%v: %v",
            cursorToken.Literal,
            s.DbtContext.VariableDetailMap[cursorToken.Literal].Value,
        )
    case parser.MACRO:
        packageName := Package(s.DbtContext.ProjectYaml.ProjectName.Value)
        prevToken := cursorTokenLL.PrevToken.PrevToken
        if prevToken.Token.Type == parser.PACKAGE {
            packageName = Package(prevToken.Token.Literal)
        }
        response.Result.Contents = s.DbtContext.MacroDetailMap[packageName][cursorToken.Literal].Description
    default:
        response.Result.Contents = dialectFunctions[cursorToken.Literal]
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

    cursorTokenLL, err := s.Documents[uri].Tokens.FindTokenAtCursor(position.Line, position.Character)
    if err != nil {
        return response
    }

    cursorToken := cursorTokenLL.Token

    switch cursorToken.Type {
    case parser.REF:
        response.Result.URI = "file://" + s.DbtContext.ModelDetailMap[cursorToken.Literal].URI
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
    case parser.VAR:
        response.Result.URI = "file://" + s.DbtContext.VariableDetailMap[cursorToken.Literal].URI
        response.Result.Range = s.DbtContext.VariableDetailMap[cursorToken.Literal].Range
    case parser.MACRO:
        packageName := Package(s.DbtContext.ProjectYaml.ProjectName.Value)
        prevToken := cursorTokenLL.PrevToken.PrevToken
        if prevToken.Token.Type == parser.PACKAGE {
            packageName = Package(prevToken.Token.Literal)
        }
        response.Result.URI = "file://" + s.DbtContext.MacroDetailMap[packageName][cursorToken.Literal].URI
        response.Result.Range = s.DbtContext.MacroDetailMap[packageName][cursorToken.Literal].Range
    default:
        response.Result.URI = uri
        line := s.Documents[uri].DefTokens[cursorToken.Literal].Line
        column := s.Documents[uri].DefTokens[cursorToken.Literal].Column
        response.Result.Range = lsp.Range{
            Start: lsp.Position{
                Line:      line,
                Character: column,
            },
            End: lsp.Position{
                Line:      line,
                Character: column,
            },
        }
    }

	return response
}

func (s *State) TextDocumentCompletion(id int, uri string, position lsp.Position) lsp.CompletionResponse {
    items := []lsp.CompletionItem{}

    fileContents := s.Documents[uri].Text
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
            s.DbtContext.VariableDetailMap,
            getVariableSuffix(textBeforeCursor, textAfterCursor),
        )
    } else if jinjaBlockRegex.MatchString(textBeforeCursor) {
        items = getMacroCompletionItems(s.DbtContext.MacroDetailMap, s.DbtContext.ProjectYaml)
    } else {
        items = s.DbtContext.Dialect.FunctionCompletionItems()
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
