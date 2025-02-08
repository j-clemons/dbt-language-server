package util

import (
	"os"
	"path/filepath"
)

func ReadFileContents(uri string) (string, error) {
    contents, err := os.ReadFile(uri)
    if err != nil {
        return "", err
    }
    return string(contents), nil
}

func GetLineAndColumn(input string, idx int) (line, column int) {
    line = 0
    lastLineIdx := 0

    for i := 0; i < idx && i < len(input); i++ {
        if input[i] == '\n' {
            line++
            lastLineIdx = i + 1
        }
    }

    column = idx - lastLineIdx
    return line, column
}

func CreateFileNameMap(fileExt string, root string, paths []string) (map[string]string, error) {
    fileMap := make(map[string]string)

    var err error

    for _, p := range paths {
        path := filepath.Join(root, p)
        _, err = os.ReadDir(path)
        if err != nil {
            continue
        }
        validPaths, err := WalkFilepath(path, fileExt)
        if err != nil {
            continue
        }

        for _, validPath := range validPaths {
            fileMap[filepath.Base(validPath)[:len(filepath.Base(validPath)) - len(fileExt)]] = validPath
        }
    }

    return fileMap, err
}
