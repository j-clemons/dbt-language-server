package util

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/j-clemons/dbt-language-server/testutils"
)

func TestFindFileDirAsc(t *testing.T) {
	expectedTestDataPath, err := testutils.GetTestdataPath("")
	expected := filepath.Join(
		expectedTestDataPath,
		"jaffle_shop_duckdb",
	)
	if err != nil {
		t.Fatal(err)
	}

	testdataPath, err := testutils.GetTestdataPath("jaffle_shop_duckdb/models")
	if err != nil {
		t.Fatal(err)
	}

	fileDir, err := findFileDir("dbt_project.yml", testdataPath)
	if err != nil {
		t.Fatal(err)
	}

	if fileDir != expected {
		t.Errorf("expected %q, got %q", expected, fileDir)
	}
}

func TestFindFileDirAscStopsAtFilesystemRoot(t *testing.T) {
	workingDir, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	root := filepath.VolumeName(workingDir) + string(filepath.Separator)
	missingFile := ".dbt-language-server-missing-project-file"

	_, err = findFileDirAsc(missingFile, root)
	if err == nil {
		t.Fatalf("expected %q lookup from %q to fail", missingFile, root)
	}
	if !strings.Contains(err.Error(), missingFile) || !strings.Contains(err.Error(), root) {
		t.Fatalf("expected descriptive root lookup error, got %v", err)
	}
}

func TestFindFileDirDescFindsNestedProject(t *testing.T) {
	testdataRoot, err := testutils.GetTestdataPath("")
	if err != nil {
		t.Fatal(err)
	}
	expected, err := testutils.GetTestdataPath("jaffle_shop_duckdb")
	if err != nil {
		t.Fatal(err)
	}

	fileDir, err := findFileDir("dbt_project.yml", testdataRoot)
	if err != nil {
		t.Fatal(err)
	}
	if fileDir != expected {
		t.Fatalf("expected nested project %q, got %q", expected, fileDir)
	}
}
