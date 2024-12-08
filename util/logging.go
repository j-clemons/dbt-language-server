package util

import (
	"log"
	"os"
	"path/filepath"
)

func GetLogger(filename string) *log.Logger {
    exePath := executableDirectory()
    logfile, err := os.OpenFile(filepath.Join(exePath, filename), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0666)
    if err != nil {
        panic("Did not provide a good file")
    }

    return log.New(logfile, "[dbt-lsp]", log.Ldate|log.Ltime|log.Lshortfile)
}

func executableDirectory() string {
    ex, err := os.Executable()
    if err != nil {
        panic(err)
    }
    exePath := filepath.Dir(ex)

    return exePath
}
