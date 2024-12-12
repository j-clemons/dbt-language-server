package testutils

import (
	"path/filepath"
	"runtime"
)

// GetTestDataPath returns the absolute path to a file in the testdata directory
// given the relative path in the testdata directory
func GetTestDataPath(relativePath string) (string, error) {
	_, filename, _, ok := runtime.Caller(1)
	if !ok {
		return "", &PathError{"unable to determine caller location"}
	}

	basePath := filepath.Join(filepath.Dir(filename), "..", "testdata")
	return filepath.Join(basePath, relativePath), nil
}

type PathError struct {
	Message string
}

func (e *PathError) Error() string {
	return e.Message
}

