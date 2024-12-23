package analysis

import (
	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/services/pb"
)

func parseSqlFluffLintResults(lintResults []*pb.LintResultItem) []lsp.Diagnostic {
    diagnostics := []lsp.Diagnostic{}

    for _, lintResultItem := range lintResults {
        severity := 1
        if lintResultItem.Warning {
            severity = 2
        }
        diagnostics = append(diagnostics, lsp.Diagnostic{
            Range: lsp.Range{
                Start: lsp.Position{
                    Line:      int(lintResultItem.StartLineNo - 1),
                    Character: int(lintResultItem.StartLinePos - 1),
                },
                End: lsp.Position{
                    Line:      int(lintResultItem.EndLineNo - 1),
                    Character: int(lintResultItem.EndLinePos - 1),
                },
            },
            Message:  lintResultItem.Description,
            Severity: severity,
            Code:     lintResultItem.Code,
            Source:   "sqlfluff",
        })
    }

    return diagnostics
}
