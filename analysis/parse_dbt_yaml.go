package analysis

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/util"
	"gopkg.in/yaml.v3"
)

type SchemaYaml struct {
    Models []Model `yaml:"models"`
}

type Model struct {
    Name        string `yaml:"name"`
    Description string `yaml:"description"`
}

type AnnotatedField[T any] struct {
	Value    T
	Position lsp.Position
}

func (a *AnnotatedField[T]) UnmarshalYAML(value *yaml.Node) error {
	a.Position = lsp.Position{
		Line:      value.Line,
		Character: value.Column,
	}
	return value.Decode(&a.Value)
}

type DbtProjectYaml struct {
	ProjectName         AnnotatedField[string]                 `yaml:"name"`
	ModelPaths          AnnotatedField[[]string]               `yaml:"model-paths"`
	MacroPaths          AnnotatedField[[]string]               `yaml:"macro-paths"`
	PackagesInstallPath AnnotatedField[string]                 `yaml:"packages-install-path"`
	DocsPaths           AnnotatedField[[]string]               `yaml:"docs-paths"`
	Vars                AnnotatedField[map[string]interface{}] `yaml:"vars"`
}

func parseDbtProjectYaml(projectRoot string) DbtProjectYaml {
    fileStr, err := util.ReadFileContents(filepath.Join(projectRoot,"dbt_project.yml"))
	if err != nil {
		fmt.Printf("Error opening file: %v\n", err)
        return DbtProjectYaml{}

	}

	var projYaml DbtProjectYaml
	if err := yaml.Unmarshal([]byte(fileStr), &projYaml); err != nil {
		fmt.Printf("Failed to unmarshal YAML: %v", err)
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

    if projYaml.ModelPaths.Value == nil || len(projYaml.ModelPaths.Value) == 0 {
        if availableDirs["models"] == 1 {
            projYaml.ModelPaths.Value = []string{"models"}
        }
    }
    if projYaml.MacroPaths.Value == nil || len(projYaml.MacroPaths.Value) == 0 {
        if availableDirs["macros"] == 1 {
            projYaml.MacroPaths.Value = []string{"macros"}
        }
    }
    if projYaml.PackagesInstallPath.Value == "" {
        if availableDirs["dbt_packages"] == 1 {
            projYaml.PackagesInstallPath.Value = "dbt_packages"
        }
    }
    if projYaml.DocsPaths.Value == nil || len(projYaml.DocsPaths.Value) == 0 {
        if availableDirs["docs"] == 1 {
            projYaml.DocsPaths.Value = []string{"docs"}
        }
        projYaml.DocsPaths.Value = append(projYaml.DocsPaths.Value, projYaml.ModelPaths.Value...)
        projYaml.DocsPaths.Value = append(projYaml.DocsPaths.Value, projYaml.MacroPaths.Value...)
    }
    return projYaml
}

func parseSchemaYamlFile(path string) SchemaYaml {
    file, err := os.Open(path)
	if err != nil {
		fmt.Printf("Error opening file: %v\n", err)
        return SchemaYaml{}

	}
	defer file.Close()

	var config SchemaYaml
	decoder := yaml.NewDecoder(file)
	if err := decoder.Decode(&config); err != nil {
		fmt.Printf("Error decoding YAML: %v\n", err)
        return SchemaYaml{}
	}
    return config
}

func parseYamlModels(projectRoot string, projYaml DbtProjectYaml) map[string]Model {
    modelMap := make(map[string]Model)

    docsFiles := getDocsFiles(projYaml)
    docsMap := processDocsFiles(docsFiles)

    for _, path := range projYaml.ModelPaths.Value {
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
