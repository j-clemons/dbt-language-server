package util

import (
	"os"
	"regexp"
	"strings"
)

func ReadFileContents(filename string) string {
    contents, err := os.ReadFile(filename)
    if err != nil {
        panic(err)
    }
    return string(contents)
}

func SplitContents(contents string) []string {
    return strings.Split(contents, "\n")
}

func GetLine(contents []string, line int) string {
    return contents[line]
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

        if len(matches) == 0 {
            return ""
        } else if matches[1] != "" {
            return matches[1]
        } else if matches[2] != "" {
            return matches[2]
        }

        return ""
}

func GetRef(uri string, line int, pos int) string {
    cleanedUri := uri[7:]

    contents := ReadFileContents(cleanedUri)
    contentSlice := SplitContents(contents)
    lineStr := GetLine(contentSlice, line)

    return getQuotedString(getString(lineStr, pos))

}

