package analysis

import (
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
        validPaths, err := walkFilepath(path, ".sql")
        if err != nil {
            continue
        }

        for _, validPath := range validPaths {
            sqlFileMap[filepath.Base(validPath)[:len(filepath.Base(validPath)) - 4]] = validPath
        }
    }

    return sqlFileMap, err
}

func createModelPathMap(projectRoot string, projYaml DbtProjectYaml) map[string]string {
    files, err := createSqlFileNameMap(projectRoot, projYaml.ModelPaths)
    if err != nil {
        log.Print(err)
        return nil
    }

    return files
}
