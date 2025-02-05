package analysis

type ModelDetails struct {
    URI         string
    ProjectName string
    Description string
}

type ProjectDetails struct {
    RootPath       string
    DbtProjectYaml DbtProjectYaml
}

func getModelDetails(projectRoot string) map[string]ModelDetails {
    modelMap := make(map[string]ModelDetails)

    dbtProjectYaml := parseDbtProjectYaml(projectRoot)
    packageDetails := getPackageModelDetails(projectRoot, dbtProjectYaml)

    processList := []ProjectDetails{
        {
            RootPath: projectRoot,
            DbtProjectYaml: dbtProjectYaml,
        },
    }
    processList = append(processList, packageDetails...)

    for _, p := range processList {
        modelPathMap := createModelPathMap(p.RootPath, p.DbtProjectYaml)
        schemaDetails := parseYamlModels(p.RootPath, p.DbtProjectYaml)

        for k, v := range modelPathMap {
            modelMapKey := k
            alias, ok := schemaDetails[k].ModelConfig["alias"].Value.(string)
            if ok && alias != "" {
                modelMapKey = alias
            }

            modelMap[modelMapKey] = ModelDetails{
                URI:         v,
                ProjectName: p.DbtProjectYaml.ProjectName.Value,
                Description: schemaDetails[k].Description.Value,
            }
        }
    }
    return modelMap
}
