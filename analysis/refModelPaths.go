package analysis

import (
    "fmt"
    "os"
    "log"
    "path/filepath"
)

func createSqlFileNameMap(root string, paths []string) (map[string]string, error) {
    sqlFileMap := make(map[string]string)

    var err error

    for _, p := range paths {
        path := root + "/" + p
        _, err = os.ReadDir(path)
        if err != nil {
            continue
        }
        err = filepath.Walk(path, func(path string, info os.FileInfo, err error) error {
            if !info.IsDir() {
                if filepath.Ext(path) == ".sql" {
                    sqlFileMap[info.Name()[:len(info.Name()) - 4]] = path
                }
            }
            return nil
        })
    }

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

func GetProjectRoot(projFile string) string {
    wd, _ := os.Getwd()
    dir, err := findFileDir(projFile, wd)
    if err != nil {
        log.Print(err)
        return ""
    }

    return dir
}

func CreateModelPathMap(projectRoot string, projYaml DbtProjectYaml) map[string]string {
    files, err := createSqlFileNameMap(projectRoot, projYaml.ModelPaths)
    if err != nil {
        log.Print(err)
        return nil
    }

    return files
}
