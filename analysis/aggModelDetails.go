package analysis

type ModelDetails struct {
    URI         string
    Description string
}

func GetModelDetails(projectRoot string) map[string]ModelDetails {
    modelMap := make(map[string]ModelDetails)

    dbtProjectYaml := parseDbtProjectYaml(projectRoot)
    modelPathMap := CreateModelPathMap(projectRoot, dbtProjectYaml)
    schemaDetails := ParseYamlModels(projectRoot, dbtProjectYaml)

    for k, v := range modelPathMap {
        modelMap[k] = ModelDetails{
            URI:         v,
            Description: schemaDetails[k].Description,
        }
    }
    return modelMap
}
