package util

import (
    "fmt"
    "os"
    "log"
    "path/filepath"
)

func findFileDir(fileName string, startPath string) (string, error) {
    path := startPath
    for {
        files, err := os.ReadDir(path)
        if err != nil {
            return "", err
        }

        for _, file := range files {
            if file.Name() == fileName {
                return path, nil
            }
        }
        if path == "/" {
            return "", fmt.Errorf("File %s not found", fileName)
        }

        path = filepath.Dir(path)
    }
}

func GetProjectRoot(projFile string) string {
    wd, _ := os.Getwd()
    dir, err := findFileDir(projFile, wd)
    if err != nil {
        log.Print(err)
        return ""
    }

    return dir
}
