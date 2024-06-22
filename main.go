package main

import (
	"bufio"
	"log"
	"os"

	"github.com/j-clemons/dbt-language-server/rpc"
)

func main() {
    logger := getLogger("/home/jclemons/Projects/dbt-lsp/log.txt")
    logger.Println("dbt Language Server Started!")
    scanner := bufio.NewScanner(os.Stdin)
    scanner.Split(rpc.Split)

    for scanner.Scan() {
        msg := scanner.Text()
        handleMessage(logger, msg)
    }
}

func handleMessage(logger *log.Logger, msg any) {
    logger.Println(msg)
}

func getLogger(filename string) *log.Logger {
    logfile, err := os.OpenFile(filename, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0666)
    if err != nil {
        panic("Did not provide a good file")
    }

    return log.New(logfile, "[dbt-lsp]", log.Ldate|log.Ltime|log.Lshortfile)
}
