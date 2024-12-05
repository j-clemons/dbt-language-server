package analysis

import (
	"fmt"
	"os"
	"path/filepath"

	"gopkg.in/yaml.v3"
)

type SchemaYaml struct {
    Models []Model `yaml:"models"`
}

type Model struct {
    Name        string `yaml:"name"`
    Description string `yaml:"description"`
}

type DbtProjectYaml struct {
    ProjectName          string   `yaml:"name"`
    SourcePaths          []string `yaml:"source-paths"`
    MacroPaths           []string `yaml:"macro-paths"`
    PackagesInstallPaths string `yaml:"packages-install-path"`
}

func parseDbtProjectYaml(projectRoot string) DbtProjectYaml {
    file, err := os.Open(projectRoot+"/dbt_project.yml")
	if err != nil {
		fmt.Printf("Error opening file: %v", err)
        return DbtProjectYaml{}

	}
	defer file.Close()

	var projYaml DbtProjectYaml
	decoder := yaml.NewDecoder(file)
	if err := decoder.Decode(&projYaml); err != nil {
		fmt.Printf("Error decoding YAML: %v", err)
        return DbtProjectYaml{}
	}
    return projYaml
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

func parseSchemaYamlFile(path string) SchemaYaml {
    file, err := os.Open(path)
	if err != nil {
		fmt.Printf("Error opening file: %v", err)
        return SchemaYaml{}

	}
	defer file.Close()

	var config SchemaYaml
	decoder := yaml.NewDecoder(file)
	if err := decoder.Decode(&config); err != nil {
		fmt.Printf("Error decoding YAML: %v", err)
        return SchemaYaml{}
	}
    return config
}

func ParseYamlModels(projectRoot string, projYaml DbtProjectYaml) map[string]Model {
    modelMap := make(map[string]Model)

    for _, path := range projYaml.SourcePaths {
        files, _ := getYamlFiles(projectRoot+"/"+path+"/")
        for _, file := range files {
            dbtYml := parseSchemaYamlFile(file)
            for _, model := range dbtYml.Models {
                modelMap[model.Name] = model
            }
        }
    }
    return modelMap
}
