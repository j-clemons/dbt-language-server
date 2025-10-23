package analysis

import (
	"fmt"
	"path/filepath"
	"regexp"
	"strings"
	"sync"

	"github.com/j-clemons/dbt-language-server/analysis/parser"
	"github.com/j-clemons/dbt-language-server/docs"
	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/util"
)

type State struct {
	Documents     map[string]Document
	DbtContext    DbtContext
	FusionEnabled bool
	FusionPath    string
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
	SourceDetailMap   map[string]Source
	MacroDetailMap    map[Package]map[string]Macro
	VariableDetailMap map[string]Variable
}

func NewState() State {
	return State{
		Documents: map[string]Document{},
		DbtContext: DbtContext{
			ProjectRoot:       "",
			ProjectYaml:       DbtProjectYaml{},
			Dialect:           "",
			ModelDetailMap:    map[string]ModelDetails{},
			SourceDetailMap:   map[string]Source{},
			MacroDetailMap:    map[Package]map[string]Macro{},
			VariableDetailMap: map[string]Variable{},
		},
		FusionEnabled: false,
		FusionPath:    "",
	}
}

func (s *State) refreshDbtContext(wd string) {
	s.DbtContext.ProjectRoot = util.GetProjectRoot("dbt_project.yml", wd)

	s.DbtContext.ProjectYaml = parseDbtProjectYaml(s.DbtContext.ProjectRoot)
	s.DbtContext.Dialect = util.GetDialect(s.DbtContext.ProjectYaml.Profile.Value, wd)

	var wg sync.WaitGroup
	wg.Add(3)

	var modelMap map[string]ModelDetails
	var sourceMap map[string]Source
	var macroMap map[Package]map[string]Macro
	var varMap map[string]Variable

	go func() {
		defer wg.Done()
		modelMap, sourceMap = s.getModelDetails()
	}()

	go func() {
		defer wg.Done()
		macroMap = s.getMacroDetails()
	}()

	go func() {
		defer wg.Done()
		varMap = s.getProjectVariables()
	}()

	wg.Wait()

	s.DbtContext.ModelDetailMap = modelMap
	s.DbtContext.SourceDetailMap = sourceMap
	s.DbtContext.MacroDetailMap = macroMap
	s.DbtContext.VariableDetailMap = varMap
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

func (s *State) UpdateDocumentIncremental(uri string, changes []lsp.TextDocumentContentChangeEvent) {
	doc, exists := s.Documents[uri]
	if !exists {
		return
	}

	currentText := doc.Text
	for _, change := range changes {
		if change.Range == (lsp.Range{}) {
			currentText = change.Text
		} else {
			currentText = s.applyIncrementalChange(currentText, change)
		}
	}

	s.parseDocument(uri, currentText)
}

func (s *State) applyIncrementalChange(text string, change lsp.TextDocumentContentChangeEvent) string {
	lines := strings.Split(text, "\n")

	startLine := change.Range.Start.Line
	startChar := change.Range.Start.Character
	endLine := change.Range.End.Line
	endChar := change.Range.End.Character

	if startLine >= len(lines) || endLine >= len(lines) {
		return text
	}

	var result strings.Builder

	for i := 0; i < startLine; i++ {
		result.WriteString(lines[i])
		result.WriteString("\n")
	}

	if startLine == endLine {
		line := lines[startLine]
		if startChar <= len(line) && endChar <= len(line) {
			newLine := line[:startChar] + change.Text + line[endChar:]
			result.WriteString(newLine)
		} else {
			result.WriteString(line)
		}
	} else {
		startLineText := ""
		if startChar <= len(lines[startLine]) {
			startLineText = lines[startLine][:startChar]
		} else {
			startLineText = lines[startLine]
		}

		endLineText := ""
		if endChar <= len(lines[endLine]) {
			endLineText = lines[endLine][endChar:]
		}

		result.WriteString(startLineText + change.Text + endLineText)
	}

	for i := endLine + 1; i < len(lines); i++ {
		result.WriteString("\n")
		result.WriteString(lines[i])
	}

	return result.String()
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
	case parser.SOURCE:
		response.Result.Contents = s.DbtContext.SourceDetailMap[cursorToken.Literal].Description
	case parser.SOURCE_TABLE:
		match, tokenLiteral := cursorTokenLL.TokenLookbackMatch(parser.SOURCE, 4)
		if match {
			source := s.DbtContext.SourceDetailMap[tokenLiteral]
			sourceTable := s.DbtContext.SourceDetailMap[tokenLiteral].Tables[cursorToken.Literal]

			response.Result.Contents = fmt.Sprintf(
				"Source: %s\n%s\n\nTable: %s\n%s",
				source.Name,
				source.Description,
				sourceTable.Name,
				sourceTable.Description,
			)
		}
	case parser.VAR:
		response.Result.Contents = fmt.Sprintf(
			"%v: %v",
			cursorToken.Literal,
			s.DbtContext.VariableDetailMap[cursorToken.Literal].Value,
		)
	case parser.MACRO:
		packageName := Package(s.DbtContext.ProjectYaml.ProjectName.Value)
		match, tokenLiteral := cursorTokenLL.TokenLookbackMatch(parser.PACKAGE, 2)
		if match {
			packageName = Package(tokenLiteral)
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
		model := s.DbtContext.ModelDetailMap[cursorToken.Literal]
		if model != (ModelDetails{}) {
			response.Result.URI = "file://" + model.URI
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
	case parser.SOURCE:
		source := s.DbtContext.SourceDetailMap[cursorToken.Literal]
		if source.Name != "" {
			response.Result.URI = "file://" + source.URI
			response.Result.Range = source.Range
		}
	case parser.SOURCE_TABLE:
		match, tokenLiteral := cursorTokenLL.TokenLookbackMatch(parser.SOURCE, 4)
		if match {
			source := s.DbtContext.SourceDetailMap[tokenLiteral].Tables[cursorToken.Literal]
			if source.Name != "" {
				response.Result.URI = "file://" + source.URI
				response.Result.Range = source.Range
			}
		}
	case parser.VAR:
		variable := s.DbtContext.VariableDetailMap[cursorToken.Literal]
		if variable != (Variable{}) {
			response.Result.URI = "file://" + variable.URI
			response.Result.Range = variable.Range
		}
	case parser.MACRO:
		packageName := Package(s.DbtContext.ProjectYaml.ProjectName.Value)
		match, tokenLiteral := cursorTokenLL.TokenLookbackMatch(parser.PACKAGE, 2)
		if match {
			packageName = Package(tokenLiteral)
		}
		macro := s.DbtContext.MacroDetailMap[packageName][cursorToken.Literal]
		if macro != (Macro{}) {
			response.Result.URI = "file://" + macro.URI
			response.Result.Range = macro.Range
		}
	default:
		response.Result.URI = uri
		defToken := s.Documents[uri].DefTokens[cursorToken.Literal]
		if defToken != (parser.Token{}) {
			response.Result.Range = lsp.Range{
				Start: lsp.Position{
					Line:      defToken.Line,
					Character: defToken.Column,
				},
				End: lsp.Position{
					Line:      defToken.Line,
					Character: defToken.Column,
				},
			}
		}
	}

	return response
}

func (s *State) GoToSchema(id int, uri string, position lsp.Position) lsp.ExecuteCommandResponse {
	response := lsp.ExecuteCommandResponse{
		Response: lsp.Response{
			RPC: "2.0",
			ID:  &id,
		},
		Result: nil,
	}

	// Check if document exists and has tokens
	doc, exists := s.Documents[uri]
	if exists && doc.Tokens != nil {
		// First, check if cursor is on a REF token
		cursorTokenLL, err := doc.Tokens.FindTokenAtCursor(position.Line, position.Character)
		if err == nil {
			cursorToken := cursorTokenLL.Token

			if cursorToken.Type == parser.REF {
				// Navigate to schema for the referenced model
				model, modelExists := s.DbtContext.ModelDetailMap[cursorToken.Literal]
				if modelExists && model.SchemaURI != "" {
					response.Result = lsp.Location{
						URI:   "file://" + model.SchemaURI,
						Range: model.SchemaRange,
					}
					return response
				}
			}
		}
	}

	// If not on a REF token, navigate to schema for current file
	// Extract model name from current file URI
	modelName := getModelNameFromURI(uri)
	if modelName != "" {
		model, modelExists := s.DbtContext.ModelDetailMap[modelName]
		if modelExists && model.SchemaURI != "" {
			response.Result = lsp.Location{
				URI:   "file://" + model.SchemaURI,
				Range: model.SchemaRange,
			}
		}
	}
	return response
}

func getModelNameFromURI(uri string) string {
	// Remove file:// prefix if present
	cleanURI := strings.TrimPrefix(uri, "file://")

	// Get the base filename without extension
	baseName := filepath.Base(cleanURI)

	// Remove .sql extension
	if strings.HasSuffix(baseName, ".sql") {
		return strings.TrimSuffix(baseName, ".sql")
	}

	return ""
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
	sourceRegex := regexp.MustCompile(`\bsource\(('|")[a-zA-z]*$`)
	varRegex := regexp.MustCompile(`\bvar\(('|")[a-zA-z]*$`)
	jinjaBlockRegex := regexp.MustCompile(`\{\{\s*`)

	if refRegex.MatchString(textBeforeCursor) {
		items = getRefCompletionItems(
			s.DbtContext.ModelDetailMap,
			getSuffix(lineText, textAfterCursor, "ref"),
		)
	} else if sourceRegex.MatchString(textBeforeCursor) {
		items = getSourceCompletionItems(
			s.DbtContext.SourceDetailMap,
			getSuffix(lineText, textAfterCursor, "source"),
			getQuoteType(lineText),
		)
	} else if varRegex.MatchString(textBeforeCursor) {
		items = getVariableCompletionItems(
			s.DbtContext.VariableDetailMap,
			getSuffix(lineText, textAfterCursor, "var"),
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
