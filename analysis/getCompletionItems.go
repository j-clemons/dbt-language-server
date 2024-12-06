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
    leadingSymbols := regexp.MustCompile(`{{\s+ref\(('|")`)
    prefix := leadingSymbols.FindString(ref)
    suffix := reverseRefPrefix(strings.Replace(prefix, "ref", "", 1))

    return suffix
}

