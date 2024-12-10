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

func createModelPathMap(projectRoot string, projYaml DbtProjectYaml) map[string]string {
    files, err := createSqlFileNameMap(projectRoot, projYaml.ModelPaths)
    if err != nil {
        log.Print(err)
        return nil
    }

    return files
}
