package analysis

func getProjectVariables(dbtProjectYaml DbtProjectYaml) map[string]interface{} {
    vars := make(map[string]interface{})

    if dbtProjectYaml.Vars == nil {
        return vars
    }

    for k, v := range dbtProjectYaml.Vars {
        if k != dbtProjectYaml.ProjectName {
           vars[k] = v
        }
    }

    projectVars, ok := dbtProjectYaml.Vars[dbtProjectYaml.ProjectName].(map[string]interface{})
    if !ok {
        return vars
    }

    for k, v := range projectVars {
        if v != nil {
            vars[k] = v
        }
    }

    return vars
}
