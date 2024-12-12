package testutils

import (
	"testing"
)

func TestGetFixturePath(t *testing.T) {
	path, err := GetTestDataPath("sample.txt")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if path == "" {
		t.Fatal("expected a valid path, got an empty string")
	}
}

