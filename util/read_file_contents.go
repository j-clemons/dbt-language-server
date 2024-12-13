package util

import (
	"os"
	"strings"
	"unicode"
)

func ReadFileContents(uri string) (string, error) {
    contents, err := os.ReadFile(uri)
    if err != nil {
        return "", err
    }
    return string(contents), nil
}

func GetStringUnderCursor(text string, line int, column int) string {
    lines := strings.Split(text, "\n")

    if line < 0 || line > len(lines) {
        return ""
    }
    currentLine := lines[line]

    if column < 0 || column > len(currentLine) {
        return ""
    }

    isWordChar := func(r rune) bool {
        return unicode.IsLetter(r) || unicode.IsNumber(r) || r == '_'
    }

    start := column
    for start > 0 && isWordChar(rune(currentLine[start-1])) {
        start--
    }

    end := column
    for end < len(currentLine) && isWordChar(rune(currentLine[end])) {
        end++
    }

    return currentLine[start:end]
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
