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

func (s *State) getProjectVariables() map[string]Variable {
    vars := make(map[string]Variable)

    if s.DbtContext.ProjectYaml.Vars == nil {
        return vars
    }

    projectUri := filepath.Join(s.DbtContext.ProjectRoot, "dbt_project.yml")

    for k, v := range s.DbtContext.ProjectYaml.Vars {
        if k != s.DbtContext.ProjectYaml.ProjectName.Value {
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

    projectVars, ok := s.DbtContext.ProjectYaml.Vars[s.DbtContext.ProjectYaml.ProjectName.Value].Value.(AnnotatedMap)
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
