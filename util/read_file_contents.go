package util

import (
	"os"
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
