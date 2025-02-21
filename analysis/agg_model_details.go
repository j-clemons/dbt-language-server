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

func (s *State) getModelDetails() (map[string]ModelDetails, map[string]Source) {
    modelMap := make(map[string]ModelDetails)
    sourceMap := make(map[string]Source)

    packageDetails := getPackageModelDetails(s.DbtContext.ProjectRoot, s.DbtContext.ProjectYaml)

    processList := []ProjectDetails{
        {
            RootPath:       s.DbtContext.ProjectRoot,
            DbtProjectYaml: s.DbtContext.ProjectYaml,
        },
    }
    processList = append(processList, packageDetails...)

    for _, p := range processList {
        modelPathMap := createModelPathMap(p.RootPath, p.DbtProjectYaml)
        modelSchemaDetails, projectSourceMap := parseYamlModels(p.RootPath, p.DbtProjectYaml)

        for k, v := range projectSourceMap {
            sourceMap[k] = v
        }

        for k, v := range modelPathMap {
            modelMapKey := k
            alias, ok := modelSchemaDetails[k].ModelConfig["alias"].Value.(string)
            if ok && alias != "" {
                modelMapKey = alias
            }

            modelMap[modelMapKey] = ModelDetails{
                URI:         v,
                ProjectName: p.DbtProjectYaml.ProjectName.Value,
                Description: modelSchemaDetails[k].Description.Value,
            }
        }
    }

    seedPathMap := createSeedPathMap(s.DbtContext.ProjectRoot, s.DbtContext.ProjectYaml)
    for k, v := range seedPathMap {
        modelMap[k] = ModelDetails{
            URI:         v,
            ProjectName: s.DbtContext.ProjectYaml.ProjectName.Value,
            Description: "Seed File",
        }
    }

    return modelMap, sourceMap
}
