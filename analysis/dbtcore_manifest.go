package analysis

import (
	"encoding/json"
	"os"
	"path/filepath"
)

// Manifest represents the dbt manifest.json artifact written by `dbt parse`
// or `dbt compile`.
type Manifest struct {
	Nodes   map[string]ManifestNode   `json:"nodes"`
	Sources map[string]ManifestSource `json:"sources"`
	Macros  map[string]ManifestMacro  `json:"macros"`
}

type ManifestNode struct {
	UniqueID         string             `json:"unique_id"`
	Name             string             `json:"name"`
	Description      string             `json:"description"`
	OriginalFilePath string             `json:"original_file_path"`
	ResourceType     string             `json:"resource_type"`
	PackageName      string             `json:"package_name"`
	PatchPath        string             `json:"patch_path"`
	Config           ManifestNodeConfig `json:"config"`
}

type ManifestNodeConfig struct {
	Alias *string `json:"alias"`
}

type ManifestSource struct {
	UniqueID         string `json:"unique_id"`
	Name             string `json:"name"`
	SourceName       string `json:"source_name"`
	Description      string `json:"description"`
	OriginalFilePath string `json:"original_file_path"`
	PackageName      string `json:"package_name"`
}

type ManifestMacro struct {
	UniqueID         string                  `json:"unique_id"`
	Name             string                  `json:"name"`
	Description      string                  `json:"description"`
	OriginalFilePath string                  `json:"original_file_path"`
	PackageName      string                  `json:"package_name"`
	Arguments        []ManifestMacroArgument `json:"arguments"`
}

type ManifestMacroArgument struct {
	Name        string `json:"name"`
	Description string `json:"description"`
}

func readManifest(projectRoot string) (*Manifest, error) {
	data, err := os.ReadFile(filepath.Join(projectRoot, "target", "manifest.json"))
	if err != nil {
		return nil, err
	}
	var m Manifest
	if err := json.Unmarshal(data, &m); err != nil {
		return nil, err
	}
	return &m, nil
}
