package docs

import (
	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/lsp/completionKind"
)

type Dialect string

func (d Dialect) FunctionDocs() map[string]string {
	switch d {
	case "snowflake":
		return SnowflakeFunctions
	default:
		return map[string]string{}
	}
}

func (d Dialect) FunctionCompletionItems() []lsp.CompletionItem {
	items := []lsp.CompletionItem{}

	functions := d.FunctionDocs()

	for k, v := range functions {
		items = append(items, lsp.CompletionItem{
			Label:         k,
			Detail:        "",
			Documentation: v,
			Kind:          completionKind.Function,
			InsertText:    k,
			SortText:      k,
		})
	}
	return items
}
