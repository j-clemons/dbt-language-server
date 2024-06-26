package main

import (
	"bufio"
	"encoding/json"
	"log"
	"os"

	"github.com/j-clemons/dbt-language-server/analysis"
	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/rpc"
)

func main() {
    logger := getLogger("/home/jclemons/Projects/dbt-lsp/log.txt")
    logger.Println("dbt Language Server Started!")
    scanner := bufio.NewScanner(os.Stdin)
    scanner.Split(rpc.Split)

    state := analysis.NewState()

    for scanner.Scan() {
        msg := scanner.Bytes()
        method, contents, err := rpc.DecodeMessage(msg)
        if err != nil {
            logger.Printf("Got an error: %s", err)
        }
        handleMessage(logger, state, method, contents)
    }
}

func handleMessage(logger *log.Logger, state analysis.State, method string, contents []byte) {
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
    case "textDocument/didOpen":
        var request lsp.DidOpenTextDocumentNotification
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/didOpen: %s", err)
            return
        }

        logger.Printf("Opened: %s\n%s", request.Params.TextDocument.URI, request.Params.TextDocument.Text)
        state.OpenDocument(request.Params.TextDocument.URI, request.Params.TextDocument.Text)
    case "textDocument/didChange":
        var request lsp.TextDocumentDidChangeNotification
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/didChange: %s", err)
            return
        }

        logger.Printf("Changed: %s", request.Params.TextDocument.URI)
        for _, change := range request.Params.ContentChanges {
            state.OpenDocument(request.Params.TextDocument.URI, change.Text)
        }
    }
}

func getLogger(filename string) *log.Logger {
    logfile, err := os.OpenFile(filename, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0666)
    if err != nil {
        panic("Did not provide a good file")
    }

    return log.New(logfile, "[dbt-lsp]", log.Ldate|log.Ltime|log.Lshortfile)
}
