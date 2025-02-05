package util

import (
	"log"
	"os"
	"path/filepath"

	"github.com/j-clemons/dbt-language-server/docs"
	"gopkg.in/yaml.v3"
)

type Profiles struct {
	DefaultTarget string            `yaml:"target"`
	Outputs       map[string]Target `yaml:"outputs"`
}

type Target struct {
    Type string `yaml:"type"`
}

func GetDialect(profileName string, inputDir string) docs.Dialect {
    filePath := ""
    if inputDir == "" {
        homeDir, err := os.UserHomeDir()
        if err != nil {
            log.Println("Error getting home directory:", err)
            return ""
        }
        filePath = filepath.Join(homeDir, ".dbt", "profiles.yml")
    } else {
        filePath = filepath.Join(inputDir, ".dbt", "profiles.yml")
    }


	data, err := os.ReadFile(filePath)
	if err != nil {
		log.Println("Error reading file:", err)
		return ""
	}

	var profilesYaml map[string]Profiles

	err = yaml.Unmarshal(data, &profilesYaml)
	if err != nil {
		log.Println("Error parsing YAML:", err)
		return ""
	}

	entry, exists := profilesYaml[profileName]
	if !exists {
		log.Println("Key not found:", profileName)
		return ""
	}

	if output, ok := entry.Outputs[entry.DefaultTarget]; ok {
        return docs.Dialect(output.Type)
	} else {
		log.Println("Default target not found in outputs")
	}

    return ""
}
