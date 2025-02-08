package analysis

import (
	"log"

	"github.com/j-clemons/dbt-language-server/util"
)

func createModelPathMap(projectRoot string, projYaml DbtProjectYaml) map[string]string {
    files, err := util.CreateFileNameMap(".sql", projectRoot, projYaml.ModelPaths.Value)
    if err != nil {
        log.Print(err)
        return nil
    }

    return files
}

func createSeedPathMap(projectRoot string, projYaml DbtProjectYaml) map[string]string {
    files, err := util.CreateFileNameMap(".csv", projectRoot, projYaml.SeedPaths.Value)
    if err != nil {
        log.Print(err)
        return nil
    }

    return files
}
