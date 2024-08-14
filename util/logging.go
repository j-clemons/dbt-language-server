package util

import (
    "log"
    "os"
)

func GetLogger(filename string) *log.Logger {
    logfile, err := os.OpenFile(filename, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0666)
    if err != nil {
        panic("Did not provide a good file")
    }

    return log.New(logfile, "[dbt-lsp]", log.Ldate|log.Ltime|log.Lshortfile)
}
