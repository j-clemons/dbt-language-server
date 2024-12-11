package analysis

import (
	"os"
	"regexp"
	"strings"
)

type Docs struct {
    Name    string
    Content string
}

func getDocsFiles(dbtProjectYaml DbtProjectYaml) []string {
    docsFiles := []string{}

    for _, path := range dbtProjectYaml.DocsPaths {
        _, err := os.ReadDir(path)
        if err != nil {
            continue
        }
        docsPaths, err := walkFilepath(path, ".md")
        if err != nil {
            continue
        }
        docsFiles = append(docsFiles, docsPaths...)
    }
    return docsFiles
}

func getDocsFileContents(docsFileStr string) []Docs {
    docs := []Docs{}

    re := regexp.MustCompile(`(?s){%-{0,1}\s*docs\s+([a-zA-z]+)\s*-{0,1}%}(.*){%-{0,1}\s*enddocs\s*%}`)
    docsMatches := re.FindAllStringSubmatch(docsFileStr, -1)
    for _, d := range docsMatches {
        docs = append(
            docs,
            Docs{
                Name:    d[1],
                Content: strings.TrimSpace(d[2]),
            },
        )
    }

    return docs
}

func makeDocsMap(docs []Docs) map[string]Docs {
    docsMap := make(map[string]Docs)
    for _, d := range docs {
        docsMap[d.Name] = d
    }
    return docsMap
}
