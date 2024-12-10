package analysis

func getProjectVariables(dbtProjectYaml DbtProjectYaml) map[string]interface{} {
    vars := make(map[string]interface{})
    for key := range dbtProjectYaml.Vars {
        if key != dbtProjectYaml.ProjectName {
           vars[key] = dbtProjectYaml.Vars[key]
        }
    }

    for key := range dbtProjectYaml.Vars[dbtProjectYaml.ProjectName].(map[string]interface{}) {
        vars[key] = dbtProjectYaml.Vars[dbtProjectYaml.ProjectName].(map[string]interface{})[key]
    }

    return vars
}
