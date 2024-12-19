package analysis

import (
	"path/filepath"

	"github.com/j-clemons/dbt-language-server/lsp"
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

    for k, v := range dbtProjectYaml.Vars {
        if k != dbtProjectYaml.ProjectName.Value {
           vars[k] = Variable{
               Name:  k,
               Value: v.Value,
               URI:   projectUri,
               Range: lsp.Range{
                   Start: v.Position,
                   End: v.Position,
               },
           }
        }
    }

    projectVars, ok := dbtProjectYaml.Vars[dbtProjectYaml.ProjectName.Value].Value.(AnnotatedMap)
    if !ok {
        return vars
    }

    for k, v := range projectVars {
        if v.Value != nil {
           vars[k] = Variable{
               Name:  k,
               Value: v.Value,
               URI:   projectUri,
               Range: lsp.Range{
                   Start: v.Position,
                   End: v.Position,
               },
           }
        }
    }

    return vars
}
