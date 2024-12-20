package main

import (
	"bufio"
	"encoding/json"
	"io"
	"log"
	"os"

	"github.com/j-clemons/dbt-language-server/analysis"
	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/rpc"
	"github.com/j-clemons/dbt-language-server/services"
	"github.com/j-clemons/dbt-language-server/services/pb"
	"github.com/j-clemons/dbt-language-server/util"
)

func main() {
    logger := util.GetLogger("log.txt")
    logger.Println("dbt Language Server Started!")

    client, err := services.PythonServer()
    if err != nil {
        logger.Printf("Error connecting to Python server: %v\n", err)
    }

    scanner := bufio.NewScanner(os.Stdin)
    scanner.Split(rpc.Split)

    state := analysis.NewState()
    writer := os.Stdout

    for scanner.Scan() {
        msg := scanner.Bytes()
        method, contents, err := rpc.DecodeMessage(msg)
        if err != nil {
            logger.Printf("Got an error: %s", err)
        }
        handleMessage(logger, writer, state, method, contents, client)
    }
}

func handleMessage(
    logger   *log.Logger,
    writer   io.Writer,
    state    analysis.State,
    method   string,
    contents []byte,
    client   pb.MyServiceClient,
 ) {
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
        writeResponse(writer, msg)

        logger.Print("Sent the reply")
    case "textDocument/didOpen":
        var request lsp.DidOpenTextDocumentNotification
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/didOpen: %s", err)
            return
        }

        state.OpenDocument(request.Params.TextDocument.URI, request.Params.TextDocument.Text)
        logger.Printf("Opened: %s\n%s", request.Params.TextDocument.URI, request.Params.TextDocument.Text)
    case "textDocument/didSave":
        logger.Print("textDocument/didSave")
        var request lsp.DidSaveTextDocumentNotification
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/didSave: %s", err)
            return
        }

        logger.Printf("Saved: %s", request.Params.TextDocument.URI)
        state.SaveDocument(request.Params.TextDocument.URI)
    case "textDocument/didChange":
        var request lsp.TextDocumentDidChangeNotification
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/didChange: %s", err)
            return
        }

        logger.Printf("Changed: %s", request.Params.TextDocument.URI)
        for _, change := range request.Params.ContentChanges {
            state.UpdateDocument(request.Params.TextDocument.URI, change.Text)
        }
    case "textDocument/hover":
        var request lsp.HoverRequest
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/hover: %s", err)
            return
        }

        response := state.Hover(request.ID, request.Params.TextDocument.URI, request.Params.Position)

        writeResponse(writer, response)
    case "textDocument/definition":
        logger.Print("textDocument/definition")
        var request lsp.DefinitionRequest
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/definition: %s", err)
            return
        }

        response := state.Definition(request.ID, request.Params.TextDocument.URI, request.Params.Position)

        writeResponse(writer, response)
    case "textDocument/completion":
        logger.Print("textDocument/completion")
        var request lsp.CompletionRequest
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/completion: %s", err)
            return
        }

        response := state.TextDocumentCompletion(request.ID, request.Params.TextDocument.URI, request.Params.Position)

        writeResponse(writer, response)
    }
}

func writeResponse(writer io.Writer, msg any) {
    reply := rpc.EncodeMessage(msg)
    writer.Write([]byte(reply))
}
