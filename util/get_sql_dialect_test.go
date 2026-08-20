package util

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func makeLocalTempDir(t *testing.T) string {
	t.Helper()

	tempDir, err := os.MkdirTemp(".", ".profiles-test-")
	if err != nil {
		t.Fatalf("create temp dir: %v", err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(tempDir); err != nil {
			t.Errorf("remove temp dir: %v", err)
		}
	})

	absoluteDir, err := filepath.Abs(tempDir)
	if err != nil {
		t.Fatalf("resolve temp dir: %v", err)
	}
	return absoluteDir
}

func writeProfiles(t *testing.T, profilesDir string, dialect string) string {
	t.Helper()

	if err := os.MkdirAll(profilesDir, 0755); err != nil {
		t.Fatalf("create profiles dir: %v", err)
	}
	profilesPath := filepath.Join(profilesDir, "profiles.yml")
	profilesContent := `test_profile:
  target: dev
  outputs:
    dev:
      type: ` + dialect + `
`
	if err := os.WriteFile(profilesPath, []byte(profilesContent), 0644); err != nil {
		t.Fatalf("write profiles.yml: %v", err)
	}
	return profilesPath
}

func TestResolveProfilesPathConfiguredDirectoryPrecedence(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	projectProfilesPath := writeProfiles(t, projectRoot, "postgres")
	configuredProfilesDir := filepath.Join(tempDir, "configured")
	configuredProfilesPath := writeProfiles(t, configuredProfilesDir, "snowflake")

	t.Setenv("DBT_PROFILES_DIR", configuredProfilesDir)

	profilesPath, err := ResolveProfilesPath(projectRoot)
	if err != nil {
		t.Fatal(err)
	}
	if profilesPath != configuredProfilesPath {
		t.Fatalf("expected %q, got %q", configuredProfilesPath, profilesPath)
	}
	if profilesPath == projectProfilesPath {
		t.Fatalf("expected configured profiles to take precedence over project profiles")
	}

	if dialect := GetDialect("test_profile", projectRoot); dialect != "snowflake" {
		t.Fatalf("expected dialect snowflake, got %q", dialect)
	}
}

func TestResolveProfilesPathRelativeConfiguredDirectory(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	t.Chdir(tempDir)

	configuredProfilesPath := writeProfiles(t, filepath.Join(tempDir, "editor-config", "dbt"), "bigquery")
	t.Setenv("DBT_PROFILES_DIR", filepath.Join("editor-config", "dbt"))

	profilesPath, err := ResolveProfilesPath(filepath.Join(tempDir, "unrelated"))
	if err != nil {
		t.Fatal(err)
	}
	if profilesPath != configuredProfilesPath {
		t.Fatalf("expected %q, got %q", configuredProfilesPath, profilesPath)
	}
}

func TestResolveProfilesPathFallbacks(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	homeDir := filepath.Join(tempDir, "home")
	t.Setenv("DBT_PROFILES_DIR", "")
	t.Setenv("HOME", homeDir)

	projectProfilesPath := writeProfiles(t, projectRoot, "bigquery")
	compatibilityProfilesPath := writeProfiles(t, filepath.Join(projectRoot, ".dbt"), "duckdb")
	homeProfilesPath := writeProfiles(t, filepath.Join(homeDir, ".dbt"), "postgres")

	profilesPath, err := ResolveProfilesPath(projectRoot)
	if err != nil {
		t.Fatal(err)
	}
	if profilesPath != projectProfilesPath {
		t.Fatalf("expected project profiles %q, got %q", projectProfilesPath, profilesPath)
	}

	if err := os.Remove(projectProfilesPath); err != nil {
		t.Fatal(err)
	}
	profilesPath, err = ResolveProfilesPath(projectRoot)
	if err != nil {
		t.Fatal(err)
	}
	if profilesPath != compatibilityProfilesPath {
		t.Fatalf("expected compatibility profiles %q, got %q", compatibilityProfilesPath, profilesPath)
	}

	if err := os.Remove(compatibilityProfilesPath); err != nil {
		t.Fatal(err)
	}
	profilesPath, err = ResolveProfilesPath(projectRoot)
	if err != nil {
		t.Fatal(err)
	}
	if profilesPath != homeProfilesPath {
		t.Fatalf("expected home profiles %q, got %q", homeProfilesPath, profilesPath)
	}
}

func TestResolveProfilesPathMissingConfiguredDirectoryDoesNotFallback(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	writeProfiles(t, projectRoot, "postgres")

	configuredProfilesDir := filepath.Join(tempDir, "missing")
	t.Setenv("DBT_PROFILES_DIR", configuredProfilesDir)

	_, err := ResolveProfilesPath(projectRoot)
	if err == nil {
		t.Fatal("expected missing configured profiles error")
	}
	if !strings.Contains(err.Error(), "DBT_PROFILES_DIR") || !strings.Contains(err.Error(), configuredProfilesDir) {
		t.Fatalf("expected descriptive configured profiles error, got %v", err)
	}
}
