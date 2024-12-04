package analysis

import "github.com/j-clemons/dbt-language-server/util"

type ModelDetails struct {
    URI         string
    Description string
}

func GetModelDetails() map[string]ModelDetails {
    modelMap := make(map[string]ModelDetails)

    modelPathMap := util.CreateModelPathMap()
    schemaDetails := ParseYamlModels()

    for k, v := range modelPathMap {
        modelMap[k] = ModelDetails{
            URI:         v,
            Description: schemaDetails[k].Description,
        }
    }
    return modelMap
}
