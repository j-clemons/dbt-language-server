package analysis

import (
	"path/filepath"
	"regexp"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/util"
)

type Variable struct {
    Name  string
    Value interface{}
    URI   string
    Range lsp.Range
}

func getProjectVariables(dbtProjectYaml DbtProjectYaml, projectRoot string) map[string]Variable {
    vars := make(map[string]Variable)

    if dbtProjectYaml.Vars == nil {
        return vars
    }

    projectUri := filepath.Join(projectRoot, "dbt_project.yml")
    fileStr, err := util.ReadFileContents(projectUri)
    if err != nil {
        fileStr = ""
    }

    for k, v := range dbtProjectYaml.Vars {
        re := regexp.MustCompile(`(?s)vars:.*?(` + k + `)`)
        matches := re.FindAllStringSubmatchIndex(fileStr, -1)
        l, c := util.GetLineAndColumn(fileStr, matches[0][1])

        if k != dbtProjectYaml.ProjectName {
           vars[k] = Variable{
               Name:  k,
               Value: v,
               URI:   projectUri,
               Range: lsp.Range{
                   Start: lsp.Position{
                       Line:      l,
                       Character: c,
                   },
                   End: lsp.Position{
                       Line:      l,
                       Character: c,
                   },
               },
           }
        }
    }

    projectVars, ok := dbtProjectYaml.Vars[dbtProjectYaml.ProjectName].(map[string]interface{})
    if !ok {
        return vars
    }

    for k, v := range projectVars {
        re := regexp.MustCompile(`(?s)vars:.*?(` + dbtProjectYaml.ProjectName + `).*?(` + k + `)`)
        matches := re.FindAllStringSubmatchIndex(fileStr, -1)
        l, c := util.GetLineAndColumn(fileStr, matches[0][1])

        if v != nil {
           vars[k] = Variable{
               Name:  k,
               Value: v,
               URI:   projectUri,
               Range: lsp.Range{
                   Start: lsp.Position{
                       Line:      l,
                       Character: c,
                   },
                   End: lsp.Position{
                       Line:      l,
                       Character: c,
                   },
               },
           }
        }
    }

    return vars
}
