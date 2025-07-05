package util

import (
	"path/filepath"
	"testing"

	"github.com/j-clemons/dbt-language-server/testutils"
)

func TestFindFileDir(t *testing.T) {
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
