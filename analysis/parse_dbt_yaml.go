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

type AnnotatedField[T any] struct {
	Value    T
	Position lsp.Position
}

func (a *AnnotatedField[T]) UnmarshalYAML(value *yaml.Node) error {
	a.Position = lsp.Position{
		Line:      value.Line - 1,
		Character: value.Column - 1,
	}
	return value.Decode(&a.Value)
}

type AnnotatedMap map[string]AnnotatedField[any]

func (a *AnnotatedMap) UnmarshalYAML(value *yaml.Node) error {
	if value.Kind != yaml.MappingNode {
		return fmt.Errorf("expected mapping node but got %v", value.Kind)
	}

    *a = make(AnnotatedMap)
	for i := 0; i < len(value.Content); i += 2 {
		keyNode := value.Content[i]
		valueNode := value.Content[i+1]

		var key string
		if err := keyNode.Decode(&key); err != nil {
			return fmt.Errorf("failed to decode key: %w", err)
		}

		if valueNode.Kind == yaml.MappingNode {
			var nested AnnotatedMap
			if err := valueNode.Decode(&nested); err != nil {
				return fmt.Errorf("failed to decode nested map for key '%s': %w", key, err)
			}

			(*a)[key] = AnnotatedField[any]{
				Value: nested,
				Position: lsp.Position{
					Line:      valueNode.Line - 1,
					Character: valueNode.Column - 1,
				},
			}
		} else {
			var value any
			if err := valueNode.Decode(&value); err != nil {
				return fmt.Errorf("failed to decode value for key '%s': %w", key, err)
			}

			(*a)[key] = AnnotatedField[any]{
				Value: value,
                Position: lsp.Position{
					Line:      valueNode.Line - 1,
                    Character: valueNode.Column - 1,
				},
			}
		}
	}
	return nil
}

type DbtProjectYaml struct {
	ProjectName         AnnotatedField[string]   `yaml:"name"`
    Profile             AnnotatedField[string]   `yaml:"profile"`
	ModelPaths          AnnotatedField[[]string] `yaml:"model-paths"`
    SeedPaths           AnnotatedField[[]string] `yaml:"seed-paths"`
	MacroPaths          AnnotatedField[[]string] `yaml:"macro-paths"`
	PackagesInstallPath AnnotatedField[string]   `yaml:"packages-install-path"`
	DocsPaths           AnnotatedField[[]string] `yaml:"docs-paths"`
	Vars                AnnotatedMap             `yaml:"vars"`
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
    if projYaml.SeedPaths.Value == nil || len(projYaml.SeedPaths.Value) == 0 {
        if availableDirs["seeds"] == 1 {
            projYaml.SeedPaths.Value = []string{"seeds"}
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

type SchemaYaml struct {
    Models []Model `yaml:"models"`
}

type Model struct {
    Name        AnnotatedField[string] `yaml:"name"`
    Description AnnotatedField[string] `yaml:"description"`
    ModelConfig AnnotatedMap           `yaml:"config"`
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
        files, _ := util.WalkFilepath(projectRoot+"/"+path+"/", ".yml")
        for _, file := range files {
            dbtYml := parseSchemaYamlFile(file)
            for _, model := range dbtYml.Models {
                modelMap[model.Name.Value] = Model{
                    Name:        model.Name,
                    Description: AnnotatedField[string]{Value: replaceDescriptionDocsBlocks(model.Description.Value, docsMap)},
                    ModelConfig: AnnotatedMap{
                        "alias": AnnotatedField[any]{
                            Value: model.ModelConfig["alias"].Value,
                            Position: lsp.Position{
                                Line:      model.ModelConfig["alias"].Position.Line,
                                Character: model.ModelConfig["alias"].Position.Character,
                            },
                        },
                    },
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
