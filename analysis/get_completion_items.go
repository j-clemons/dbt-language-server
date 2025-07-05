package analysis

import (
	"fmt"
	"regexp"
	"strings"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/lsp/completionKind"
)

func getRefCompletionItems(modelMap map[string]ModelDetails, suffix string) []lsp.CompletionItem {
	items := make([]lsp.CompletionItem, 0, len(modelMap))

	for k := range modelMap {
		items = append(
			items,
			lsp.CompletionItem{
				Label:         k,
				Detail:        fmt.Sprintf("Project: %s", modelMap[k].ProjectName),
				Documentation: modelMap[k].Description,
				Kind:          completionKind.Reference,
				InsertText:    fmt.Sprintf("%s%s", k, suffix),
				SortText:      k,
			},
		)
	}

	return items
}

func reverseRefPrefix(str string) string {
	var result string
	for _, v := range str {
		switch v {
		case '(':
			result = ")" + result
		case '{':
			result = "}" + result
		default:
			result = string(v) + result
		}
	}

	return result
}

func getSuffix(leadingStr string, trailingStr string, suffixType string) string {
	if trailingStr != "" {
		return ""
	}
	leadingSymbols := regexp.MustCompile(`{{\s*` + suffixType + `\(('|")`)
	prefix := leadingSymbols.FindString(leadingStr)
	suffix := reverseRefPrefix(strings.Replace(prefix, suffixType, "", 1))

	return suffix
}

// Get the last used quote type in the string
func getQuoteType(str string) string {
	quotes := regexp.MustCompile(`['"]`)
	allMatches := quotes.FindString(str)
	if len(allMatches) == 0 {
		return ""
	}
	return string(allMatches[len(allMatches)-1])
}

func getMacroCompletionItems(packageMacroMap map[Package]map[string]Macro, ProjectYaml DbtProjectYaml) []lsp.CompletionItem {
	items := make([]lsp.CompletionItem, 0, len(packageMacroMap))

	for _, macroMap := range packageMacroMap {
		for k := range macroMap {
			var insertText string
			if ProjectYaml.ProjectName.Value == string(macroMap[k].ProjectName) {
				insertText = k
			} else {
				insertText = fmt.Sprintf("%s.%s", macroMap[k].ProjectName, k)
			}

			items = append(
				items,
				lsp.CompletionItem{
					Label:         k,
					Detail:        fmt.Sprintf("Project: %s", macroMap[k].ProjectName),
					Documentation: macroMap[k].Description,
					Kind:          completionKind.Snippet,
					InsertText:    insertText,
					SortText:      k,
				},
			)
		}
	}

	return items
}

func getVariableCompletionItems(variables map[string]Variable, suffix string) []lsp.CompletionItem {
	items := make([]lsp.CompletionItem, 0, len(variables))

	for k, v := range variables {
		items = append(
			items,
			lsp.CompletionItem{
				Label:         k,
				Detail:        k,
				Documentation: fmt.Sprintf("%v", v.Value),
				Kind:          completionKind.Variable,
				InsertText:    fmt.Sprintf("%s%s", k, suffix),
				SortText:      k,
			},
		)
	}

	return items
}

func getSourceCompletionItems(sources map[string]Source, suffix string, quoteType string) []lsp.CompletionItem {
	items := make([]lsp.CompletionItem, 0, len(sources))

	for k, s := range sources {
		for _, t := range s.Tables {
			items = append(
				items,
				lsp.CompletionItem{
					Label:         fmt.Sprintf("%s - %s", s.Name, t.Name),
					Detail:        fmt.Sprintf("Source: %s", s.Name),
					Documentation: fmt.Sprintf("%s\n\nTable: %s\n%s", s.Description, t.Name, t.Description),
					Kind:          completionKind.Reference,
					InsertText:    fmt.Sprintf("%s%s, %s%s%s", s.Name, quoteType, quoteType, t.Name, suffix),
					SortText:      k,
				},
			)
		}
	}

	return items
}
