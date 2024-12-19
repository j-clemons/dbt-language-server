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
        ProjectDetails{
            RootPath: projectRoot,
            DbtProjectYaml: dbtProjectYaml,
        },
    }
    processList = append(processList, packageDetails...)

    for _, p := range processList {
        modelPathMap := createModelPathMap(p.RootPath, p.DbtProjectYaml)
        schemaDetails := parseYamlModels(p.RootPath, p.DbtProjectYaml)

        for k, v := range modelPathMap {
            modelMap[k] = ModelDetails{
                URI:         v,
                ProjectName: p.DbtProjectYaml.ProjectName.Value,
                Description: schemaDetails[k].Description,
            }
        }
    }
    return modelMap
}
