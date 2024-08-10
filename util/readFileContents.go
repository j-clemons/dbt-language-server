package util

import (
    "os"
    "strings"
    "regexp"
)

func ReadFileContents(filename string) string {
    contents, err := os.ReadFile(filename)
    if err != nil {
        panic(err)
    }
    return string(contents)
}

func splitContents(contents string) []string {
    return strings.Split(contents, "\n")
}

func getLine(contents []string, line int) string {
    return contents[line - 1]
}

func getString(s string, pos int) string {
    re := regexp.MustCompile(`\S+`)
    strRng := re.FindAllIndex([]byte(s), -1)

    if len(strRng) == 0 {
        return ""
    } else if len(strRng) == 1 {
        return s[strRng[0][0]:strRng[0][1]]
    }

    for _, r := range strRng {
        if pos >= r[0] && pos < r[1] {
            return s[r[0]:r[1]]
        }
    }

    return ""
}

func getQuotedString(s string) string {
        re := regexp.MustCompile(`"(.+)"|'(.+)'`)
        matches := re.FindStringSubmatch(s)

        return matches[len(matches) - 1]
}

// check if string contains ref(
// if yes then extract the string in the parentheses and remove quotes
