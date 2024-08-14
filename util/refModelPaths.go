package util

import (
    "fmt"
    "os"
    "log"
    "path/filepath"
)

func createSqlFileNameMap(path string) (map[string]string, error) {
    sqlFileMap := make(map[string]string)
    err := filepath.Walk(path, func(path string, info os.FileInfo, err error) error {
        if !info.IsDir() {
            if filepath.Ext(path) == ".sql" {
                sqlFileMap[info.Name()[:len(info.Name()) - 4]] = path
            }
        }
        return nil
    })

    return sqlFileMap, err
}

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

func CreateModelPathMap() map[string]string {
    wd, _ := os.Getwd()
    dir, _ := findFileDir("dbt_project.yml", wd)

    files, err := createSqlFileNameMap(dir+"/models/")
    if err != nil {
        log.Print(err)
    }

    return files
}

