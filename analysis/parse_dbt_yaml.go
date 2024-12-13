package analysis

import (
	"fmt"
	"os"
	"regexp"

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
    ProjectName         string                 `yaml:"name"`
    ModelPaths          []string               `yaml:"model-paths"`
    MacroPaths          []string               `yaml:"macro-paths"`
    PackagesInstallPath string                 `yaml:"packages-install-path"`
    DocsPaths           []string               `yaml:"docs-paths"`
    Vars                map[string]interface{} `yaml:"vars"`
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

    availableDirs := map[string]int{}
    entries, err := os.ReadDir(projectRoot)
	if err == nil {
        for _, entry := range entries {
            if entry.IsDir() {
                availableDirs[entry.Name()] = 1
            }
        }
	}

    if projYaml.ModelPaths == nil || len(projYaml.ModelPaths) == 0 {
        if availableDirs["models"] == 1 {
            projYaml.ModelPaths = []string{"models"}
        }
    }
    if projYaml.MacroPaths == nil || len(projYaml.MacroPaths) == 0 {
        if availableDirs["macros"] == 1 {
            projYaml.MacroPaths = []string{"macros"}
        }
    }
    if projYaml.PackagesInstallPath == "" {
        if availableDirs["dbt_packages"] == 1 {
            projYaml.PackagesInstallPath = "dbt_packages"
        }
    }
    if projYaml.DocsPaths == nil || len(projYaml.DocsPaths) == 0 {
        if availableDirs["docs"] == 1 {
            projYaml.DocsPaths = []string{"docs"}
        }
        projYaml.DocsPaths = append(projYaml.DocsPaths, projYaml.ModelPaths...)
        projYaml.DocsPaths = append(projYaml.DocsPaths, projYaml.MacroPaths...)
    }
    return projYaml
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

func parseYamlModels(projectRoot string, projYaml DbtProjectYaml) map[string]Model {
    modelMap := make(map[string]Model)

    docsFiles := getDocsFiles(projYaml)
    docsMap := processDocsFiles(docsFiles)

    for _, path := range projYaml.ModelPaths {
        _, err := os.ReadDir(projectRoot+"/"+path)
        if err != nil {
            continue
        }
        files, _ := walkFilepath(projectRoot+"/"+path+"/", ".yml")
        for _, file := range files {
            dbtYml := parseSchemaYamlFile(file)
            for _, model := range dbtYml.Models {
                modelMap[model.Name] = Model{
                    Name:        model.Name,
                    Description: replaceDescriptionDocsBlocks(model.Description, docsMap),
                }
            }
        }
    }

    return modelMap
}

func replaceDescriptionDocsBlocks(description string, docsMap map[string]Docs) string {
    docBlocksRegex := regexp.MustCompile(`{{\s*doc\(('|")([-zA-z]+)('|")\)\s*}}`)

    matches := docBlocksRegex.FindAllStringSubmatchIndex(description, -1)
    if len(matches) == 0 {
        return description
    }

    newDescription := description
    for i := 0; i < len(matches); i++ {
        docName := description[matches[i][4]:matches[i][5]]

        if _, ok := docsMap[docName]; ok {
            newDescription = newDescription[:matches[i][0]] + docsMap[docName].Content + newDescription[matches[i][1]:]
        }
    }

    return newDescription
}
