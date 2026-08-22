package util

import (
	"os"
	"path/filepath"
	"testing"
)

func TestGetDialect_CustomProfilesDir(t *testing.T) {
	// Create a temporary directory for testing
	tempDir, err := os.MkdirTemp("", "dbt_test")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tempDir)

	// Create custom profiles directory (absolute path)
	customProfilesDir := filepath.Join(tempDir, "custom_dbt_config")
	err = os.MkdirAll(customProfilesDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create custom profiles dir: %v", err)
	}

	// Create a test profiles.yml file
	profilesContent := `test_profile:
  target: dev
  outputs:
    dev:
      type: snowflake
`
	profilesFile := filepath.Join(customProfilesDir, "profiles.yml")
	err = os.WriteFile(profilesFile, []byte(profilesContent), 0644)
	if err != nil {
		t.Fatalf("Failed to write profiles.yml: %v", err)
	}

	// Set DBT_ENGINE_PROFILES_DIR to absolute path
	t.Setenv("DBT_ENGINE_PROFILES_DIR", customProfilesDir)

	// Test GetDialect with custom profiles directory
	dialect := GetDialect("test_profile", tempDir)

	if dialect != "snowflake" {
		t.Errorf("Expected dialect 'snowflake', got '%s'", dialect)
	}
}

func TestGetDialect_DefaultProfilesDir(t *testing.T) {
	// Ensure no custom dir is set
	t.Setenv("DBT_ENGINE_PROFILES_DIR", "")
	t.Setenv("DBT_PROFILES_DIR", "")

	// Create a temporary directory for testing
	tempDir, err := os.MkdirTemp("", "dbt_test")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tempDir)

	// Create default .dbt directory
	dbtDir := filepath.Join(tempDir, ".dbt")
	err = os.MkdirAll(dbtDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create .dbt dir: %v", err)
	}

	// Create a test profiles.yml file
	profilesContent := `test_profile:
  target: dev
  outputs:
    dev:
      type: postgres
`
	profilesFile := filepath.Join(dbtDir, "profiles.yml")
	err = os.WriteFile(profilesFile, []byte(profilesContent), 0644)
	if err != nil {
		t.Fatalf("Failed to write profiles.yml: %v", err)
	}

	t.Setenv("HOME", tempDir)

	// Test GetDialect with default profiles directory
	dialect := GetDialect("test_profile", t.TempDir())

	if dialect != "postgres" {
		t.Errorf("Expected dialect 'postgres', got '%s'", dialect)
	}
}

func TestGetDialect_ProjectRootProfiles(t *testing.T) {
	projectRoot := t.TempDir()
	t.Setenv("DBT_ENGINE_PROFILES_DIR", "")
	t.Setenv("DBT_PROFILES_DIR", "")

	profilesContent := `test_profile:
  target: dev
  outputs:
    dev:
      type: duckdb
`
	profilesFile := filepath.Join(projectRoot, "profiles.yml")
	if err := os.WriteFile(profilesFile, []byte(profilesContent), 0644); err != nil {
		t.Fatalf("Failed to write profiles.yml: %v", err)
	}

	dialect := GetDialect("test_profile", projectRoot)

	if dialect != "duckdb" {
		t.Errorf("Expected dialect 'duckdb', got '%s'", dialect)
	}
}

func TestGetProfilesPath_DbtEngineProfilesDirTakesPrecedence(t *testing.T) {
	dbtEngineProfilesDir := t.TempDir()
	t.Setenv("DBT_ENGINE_PROFILES_DIR", dbtEngineProfilesDir)
	t.Setenv("DBT_PROFILES_DIR", t.TempDir())

	profilesPath, err := getProfilesPath(t.TempDir())
	if err != nil {
		t.Fatalf("Failed to resolve profiles path: %v", err)
	}

	expectedPath := filepath.Join(dbtEngineProfilesDir, "profiles.yml")
	if profilesPath != expectedPath {
		t.Errorf("Expected profiles path '%s', got '%s'", expectedPath, profilesPath)
	}
}

func TestGetProfilesPath_LegacyDbtProfilesDir(t *testing.T) {
	// clear this out first so the legacy environment variable takes effect
	t.Setenv("DBT_ENGINE_PROFILES_DIR", "")

	dbtProfilesDir := t.TempDir()
	t.Setenv("DBT_PROFILES_DIR", dbtProfilesDir)

	profilesPath, err := getProfilesPath(t.TempDir())
	if err != nil {
		t.Fatalf("Failed to resolve profiles path: %v", err)
	}

	expectedPath := filepath.Join(dbtProfilesDir, "profiles.yml")
	if profilesPath != expectedPath {
		t.Errorf("Expected profiles path '%s', got '%s'", expectedPath, profilesPath)
	}
}
