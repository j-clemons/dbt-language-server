package testutils

import (
	"testing"
)

func TestGetTestdataPath(t *testing.T) {
	path, err := GetTestdataPath("sample.txt")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if path == "" {
		t.Fatal("expected a valid path, got an empty string")
	}
}
