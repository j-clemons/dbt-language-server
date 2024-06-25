package main

import (
	"bufio"
	"encoding/json"
	"log"
	"os"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/rpc"
)

func main() {
    logger := getLogger("/home/jclemons/Projects/dbt-lsp/log.txt")
    logger.Println("dbt Language Server Started!")
    scanner := bufio.NewScanner(os.Stdin)
    scanner.Split(rpc.Split)

    for scanner.Scan() {
        msg := scanner.Bytes()
        method, contents, err := rpc.DecodeMessage(msg)
        if err != nil {
            logger.Printf("Got an error: %s", err)
        }
        handleMessage(logger, method, contents)
    }
}

func handleMessage(logger *log.Logger, method string, contents []byte) {
    logger.Printf("Received msg with method: %s", method)

    switch method {
    case "initialize":
        var request lsp.InitializeRequest
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("Could not parse: %s", err)
        }

        logger.Printf("Connected to: %s %s",
            request.Params.ClientInfo.Name,
            request.Params.ClientInfo.Version)

        msg := lsp.NewInitializeResponse(request.ID)
        reply := rpc.EncodeMessage(msg)

        writer := os.Stdout
        writer.Write([]byte(reply))

        logger.Print("Sent the reply")
    }
}

func getLogger(filename string) *log.Logger {
    logfile, err := os.OpenFile(filename, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0666)
    if err != nil {
        panic("Did not provide a good file")
    }

    return log.New(logfile, "[dbt-lsp]", log.Ldate|log.Ltime|log.Lshortfile)
}
