package analysis

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/util"
)

type Macro struct {
    Name        string
    ProjectName string
    Description string
    URI         string
    Range       lsp.Range
}

func getMacrosFromFile(fileStr string, fileUri string, dbtProjectYaml DbtProjectYaml) []Macro {
    macroDescRegex := regexp.MustCompile(`(?s)\{%-{0,1}\s*macro\s+(\w+\(.*?\))\s*-{0,1}%\}`)
    macroMatches := macroDescRegex.FindAllStringSubmatchIndex(fileStr, -1)

    macros := []Macro{}
    for _, m := range macroMatches {
        macroNameIdx := strings.Index(fileStr[m[2]:m[3]], "(")
        if macroNameIdx == -1 {
            continue
        }
        startLine, startCol := util.GetLineAndColumn(fileStr, m[2])
        endLine, endCol := util.GetLineAndColumn(fileStr, m[3])
        macros = append(
            macros,
            Macro{
                Name:        fileStr[m[2]:m[3]][:macroNameIdx],
                ProjectName: dbtProjectYaml.ProjectName,
                Description: fileStr[m[2]:m[3]],
                URI:         fileUri,
                Range:       lsp.Range{
                    Start: lsp.Position{
                        Line:      startLine,
                        Character: startCol,
                    },
                    End: lsp.Position{
                        Line:      endLine,
                        Character: endCol,
                    },
                },
            },
        )
    }
    return macros
}

func parseMacros(projectRoot string, dbtProjectYaml DbtProjectYaml) ([]Macro, error) {
    macros := []Macro{}

    var err error
    for _, p := range dbtProjectYaml.MacroPaths {
        path := projectRoot + "/" + p
        _, err = os.ReadDir(path)
        if err != nil {
            continue
        }
        err = filepath.Walk(path, func(path string, info os.FileInfo, err error) error {
            if !info.IsDir() {
                if filepath.Ext(path) == ".sql" {
                    fileContents, err := util.ReadFileContents(path)
                    if err != nil {
                        return nil
                    }
                    macros = append(macros, getMacrosFromFile(fileContents, path, dbtProjectYaml)...)
                }
            }
            return nil
        })
    }
    return macros, err
}
