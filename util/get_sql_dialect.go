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

func ResolveConfiguredProfilesDir() (string, bool, error) {
	configuredDir := os.Getenv("DBT_PROFILES_DIR")
	if configuredDir == "" {
		return "", false, nil
	}

	if filepath.IsAbs(configuredDir) {
		return configuredDir, true, nil
	}

	cwd, err := os.Getwd()
	if err != nil {
		return "", true, fmt.Errorf("resolve relative DBT_PROFILES_DIR %q: %w", configuredDir, err)
	}

	return filepath.Join(cwd, configuredDir), true, nil
}

func ResolveProfilesPath(inputDir string) (string, error) {
	configuredDir, configured, err := ResolveConfiguredProfilesDir()
	if err != nil {
		return "", err
	}
	if configured {
		profilesPath := filepath.Join(configuredDir, "profiles.yml")
		exists, err := profilesFileExists(profilesPath)
		if err != nil {
			return "", err
		}
		if !exists {
			return "", fmt.Errorf("DBT_PROFILES_DIR %q does not contain profiles.yml", configuredDir)
		}
		return profilesPath, nil
	}

	candidates := []string{}
	if inputDir != "" {
		candidates = append(
			candidates,
			filepath.Join(inputDir, "profiles.yml"),
			filepath.Join(inputDir, ".dbt", "profiles.yml"),
		)
	}

	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("resolve home directory for dbt profiles: %w", err)
	}
	candidates = append(candidates, filepath.Join(homeDir, ".dbt", "profiles.yml"))

	for _, candidate := range candidates {
		exists, err := profilesFileExists(candidate)
		if err != nil {
			return "", err
		}
		if exists {
			return candidate, nil
		}
	}

	return "", fmt.Errorf("profiles.yml not found; searched %v", candidates)
}

func profilesFileExists(path string) (bool, error) {
	fileInfo, err := os.Stat(path)
	if err == nil {
		if fileInfo.IsDir() {
			return false, fmt.Errorf("dbt profiles path %q is a directory", path)
		}
		return true, nil
	}
	if os.IsNotExist(err) {
		return false, nil
	}

	return false, fmt.Errorf("inspect dbt profiles path %q: %w", path, err)
}

func GetDialect(profileName string, inputDir string) docs.Dialect {
	filePath, err := ResolveProfilesPath(inputDir)
	if err != nil {
		log.Printf("Failed to resolve profiles.yml for profile %q: %v", profileName, err)
		return ""
	}

	data, err := os.ReadFile(filePath)
	if err != nil {
		log.Printf("Failed to read dbt profiles at %q: %v", filePath, err)
		return ""
	}

	var profilesYaml map[string]Profiles

	err = yaml.Unmarshal(data, &profilesYaml)
	if err != nil {
		log.Printf("Failed to parse dbt profiles at %q: %v", filePath, err)
		return ""
	}

	entry, exists := profilesYaml[profileName]
	if !exists {
		log.Printf("Profile %q not found in %q", profileName, filePath)
		return ""
	}

	if output, ok := entry.Outputs[entry.DefaultTarget]; ok {
		return docs.Dialect(output.Type)
	} else {
		log.Printf("Default target %q for profile %q not found in outputs", entry.DefaultTarget, profileName)
	}

	return ""
}
