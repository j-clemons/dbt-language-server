package analysis

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/j-clemons/dbt-language-server/util"
	"gopkg.in/yaml.v3"
)

type DbtYaml struct {
    Models []Model `yaml:"models"`
}

type Model struct {
    Name        string `yaml:"name"`
    Description string `yaml:"description"`
}

func getYamlFiles(path string) ([]string, error) {
    files := make([]string, 0)
    err := filepath.Walk(path, func(path string, info os.FileInfo, err error) error {
        if !info.IsDir() {
            if filepath.Ext(path) == ".yml" || filepath.Ext(path) == ".yaml" {
                files = append(files, path)
            }
        }
        return nil
    })

    return files, err
}

func parseYamlFile(path string) DbtYaml {
    file, err := os.Open(path)
	if err != nil {
		fmt.Printf("Error opening file: %v", err)
        return DbtYaml{}

	}
	defer file.Close()

	var config DbtYaml
	decoder := yaml.NewDecoder(file)
	if err := decoder.Decode(&config); err != nil {
		fmt.Printf("Error decoding YAML: %v", err)
        return DbtYaml{}
	}
    return config
}

func ParseYamlModels() map[string]Model {
    modelMap := make(map[string]Model)
    root := util.GetProjectRoot("dbt_project.yml")
    files, _ := getYamlFiles(root+"/models/")
    for _, file := range files {
        dbtYml := parseYamlFile(file)
        for _, model := range dbtYml.Models {
            modelMap[model.Name] = model
        }
    }
    return modelMap
}
