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

func GetModelDetails(projectRoot string) map[string]ModelDetails {
    modelMap := make(map[string]ModelDetails)

    dbtProjectYaml := ParseDbtProjectYaml(projectRoot)
    packageDetails := getPackageModelDetails(projectRoot, dbtProjectYaml)

    processList := []ProjectDetails{}
    processList = append(
        processList,
        ProjectDetails{
            RootPath: projectRoot,
            DbtProjectYaml: dbtProjectYaml,
        },
    )
    processList = append(processList, packageDetails...)

    for _, p := range processList {
        modelPathMap := CreateModelPathMap(p.RootPath, p.DbtProjectYaml)
        schemaDetails := ParseYamlModels(p.RootPath, p.DbtProjectYaml)

        for k, v := range modelPathMap {
            modelMap[k] = ModelDetails{
                URI:         v,
                ProjectName: p.DbtProjectYaml.ProjectName,
                Description: schemaDetails[k].Description,
            }
        }
    }
    return modelMap
}
