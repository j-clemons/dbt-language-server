package util

import (
	"fmt"
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

func GetDialect(profileName string, projectRoot string) docs.Dialect {
	filePath, err := getProfilesPath(projectRoot)
	if err != nil {
		log.Println("Error resolving profiles.yml path:", err)
		return ""
	}

	data, err := os.ReadFile(filePath)
	if err != nil {
		log.Printf("Error reading profiles file %s: %v", filePath, err)
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

func getProfilesPath(projectRoot string) (string, error) {
	// Based on documented profiles.yml location hierarchy
	// https://docs.getdbt.com/docs/local/profiles.yml?version=2#location-of-profilesyml
	if dbtEngineProfilesDir := os.Getenv("DBT_ENGINE_PROFILES_DIR"); dbtEngineProfilesDir != "" {
		return filepath.Join(dbtEngineProfilesDir, "profiles.yml"), nil
	}

	if dbtProfilesDir := os.Getenv("DBT_PROFILES_DIR"); dbtProfilesDir != "" {
		return filepath.Join(dbtProfilesDir, "profiles.yml"), nil
	}

	if projectRoot != "" {
		projectPath := filepath.Join(projectRoot, "profiles.yml")
		_, err := os.Stat(projectPath)
		if err == nil {
			return projectPath, nil
		}
		if !os.IsNotExist(err) {
			return "", fmt.Errorf("checking profiles file %s: %w", projectPath, err)
		}
	}

	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("getting home directory: %w", err)
	}

	return filepath.Join(homeDir, ".dbt", "profiles.yml"), nil
}
