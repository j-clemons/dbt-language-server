package analysis

import (
	"github.com/j-clemons/dbt-language-server/lsp"
)

func GetRefCompletionItems(pathMap map[string]string) []lsp.CompletionItem {
    items := make([]lsp.CompletionItem, 0, len(pathMap))

    for k := range pathMap {
        items = append(items, lsp.CompletionItem{Label: k})
    }
    return items
}
