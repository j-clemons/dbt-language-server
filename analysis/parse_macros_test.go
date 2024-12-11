package analysis

import (
	"testing"

	"github.com/j-clemons/dbt-language-server/lsp"
)

func TestGetMacrosFromFile(t *testing.T) {
    macroFileStr := `
{% macro example_macro(str) %}

{% endmacro %}

{% macro multiline_macro(
    str,
    int
) %}

{% endmacro %}
`
    testCases := []struct {
        name           string
        fileStr        string
        fileUri        string
        dbtProjectYaml DbtProjectYaml
        expected       []Macro
    }{
        {
            name:     "Example File",
            fileStr:  macroFileStr,
            fileUri:  "file:///path/to/file.sql",
            dbtProjectYaml: DbtProjectYaml{
                ProjectName: "example",
                MacroPaths: []string{
                    "macros",
                },
            },
            expected: []Macro{
                Macro{
                    Name:        "example_macro",
                    ProjectName: "example",
                    Description: "example_macro(str)",
                    URI:         "file:///path/to/file.sql",
                    Range:       lsp.Range{
                        Start: lsp.Position{
                            Line:      1,
                            Character: 9,
                        },
                        End: lsp.Position{
                            Line:      1,
                            Character: 27,
                        },
                    },
                },
                Macro{
                    Name:        "multiline_macro",
                    ProjectName: "example",
                    Description: "multiline_macro(\n    str,\n    int\n)",
                    URI:         "file:///path/to/file.sql",
                    Range:       lsp.Range{
                        Start: lsp.Position{
                            Line:      5,
                            Character: 9,
                        },
                        End: lsp.Position{
                            Line:      8,
                            Character: 1,
                        },
                    },
                },
            },
        },
    }

    for _, tc := range testCases {
        t.Run(tc.name, func(t *testing.T) {
            result := getMacrosFromFile(tc.fileStr, tc.fileUri, tc.dbtProjectYaml)
            for i, e := range tc.expected {
                if e != result[i] {
                    t.Errorf("input: %v; got: %v; want: %v",
                        tc.fileStr, result[i], e)
                }
            }
        })
    }
}
