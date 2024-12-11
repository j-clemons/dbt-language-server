package analysis

import (
	"os"
	"path/filepath"
)

func walkFilepath(path string, fileExt string) ([]string, error) {
    validPaths := []string{}
    err := filepath.Walk(path, func(path string, info os.FileInfo, err error) error {
        if !info.IsDir() {
            if filepath.Ext(path) == fileExt {
                validPaths = append(validPaths, path)
            }
        }
        return nil
    })

    return validPaths, err
}
