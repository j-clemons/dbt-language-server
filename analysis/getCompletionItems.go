package analysis

import (
	"fmt"
	"regexp"
	"strings"

	"github.com/j-clemons/dbt-language-server/lsp"
)

func GetRefCompletionItems(modelMap map[string]ModelDetails, suffix string) []lsp.CompletionItem {
    items := make([]lsp.CompletionItem, 0, len(modelMap))

    for k := range modelMap {
        items = append(
            items,
            lsp.CompletionItem{
                Label:         k,
                Detail:        fmt.Sprintf("Project: %s", modelMap[k].ProjectName),
                Documentation: modelMap[k].Description,
                Kind:          18,
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

func GetReferenceSuffix(ref string) string {
    leadingAndTrailingSymbols := regexp.MustCompile(`{{\s*ref\(('|")[a-zA-z]*('|")\)\s*}}`)
    if leadingAndTrailingSymbols.MatchString(ref) {
        return ""
    }
    leadingSymbols := regexp.MustCompile(`{{\s*ref\(('|")`)
    prefix := leadingSymbols.FindString(ref)
    suffix := reverseRefPrefix(strings.Replace(prefix, "ref", "", 1))

    return suffix
}

func GetMacroCompletionItems(macroMap map[string]Macro, ProjectYaml DbtProjectYaml) []lsp.CompletionItem {
    items := make([]lsp.CompletionItem, 0, len(macroMap))

    for k := range macroMap {
        var insertText string
        if ProjectYaml.ProjectName == macroMap[k].ProjectName {
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
                Kind:          15,
                InsertText:    insertText,
                SortText:      k,
            },
        )
    }

    return items
}

func getVariableSuffix(vars string) string {
    leadingAndTrailingSymbols := regexp.MustCompile(`{{\s*var\(('|")[a-zA-z]*('|")\)\s*}}`)
    if leadingAndTrailingSymbols.MatchString(vars) {
        return ""
    }
    leadingSymbols := regexp.MustCompile(`{{\s*var\(('|")`)
    prefix := leadingSymbols.FindString(vars)
    suffix := reverseRefPrefix(strings.Replace(prefix, "var", "", 1))

    return suffix
}

func GetVariableCompletionItems(variables map[string]interface{}, suffix string) []lsp.CompletionItem {
    items := make([]lsp.CompletionItem, 0, len(variables))

    for k, v := range variables {
        items = append(
            items,
            lsp.CompletionItem{
                Label:         k,
                Detail:        k,
                Documentation: fmt.Sprintf("%v", v),
                Kind:          6,
                InsertText:    fmt.Sprintf("%s%s", k, suffix),
                SortText:      k,
            },
        )
    }

    return items
}
