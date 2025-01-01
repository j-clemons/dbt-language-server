package main

import (
	"bufio"
    "context"
	"encoding/json"
	"io"
	"log"
	"os"
    "os/signal"
    "sync"
    "syscall"
    "time"

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

    // Create a context that can be canceled
    _, cancel := context.WithCancel(context.Background())
    defer cancel()

    // Create a WaitGroup to ensure clean shutdown
    var wg sync.WaitGroup

    // Set up signal handling
    sigChan := make(chan os.Signal, 1)
    signal.Notify(sigChan, os.Interrupt, syscall.SIGTERM)

    // Start Python server with context
    pythonCmd, err := services.StartPythonServer()
    if err != nil {
        logger.Printf("Error starting Python server: %v\n", err)
        return
    }

    // Create process termination channel
    done := make(chan error, 1)
    go func() {
        done <- pythonCmd.Wait()
    }()

    // Set up clean shutdown function
    shutdownServer := func() {
        logger.Println("Initiating shutdown sequence...")

        // Cancel context
        cancel()

        // First try graceful shutdown via SIGTERM
        if err := pythonCmd.Process.Signal(syscall.SIGTERM); err != nil {
            logger.Printf("Error sending SIGTERM: %v\n", err)
        }

        // Wait for graceful shutdown with timeout
        select {
        case <-done:
            logger.Println("Python server terminated gracefully")
        case <-time.After(5 * time.Second):
            logger.Println("Shutdown timeout reached, forcing termination")
            if err := pythonCmd.Process.Kill(); err != nil {
                logger.Printf("Error killing process: %v\n", err)
            }
            // Wait for the process to be killed
            <-done
        }

        // Kill process group if process is still running
        if err := syscall.Kill(-pythonCmd.Process.Pid, syscall.SIGKILL); err != nil {
            logger.Printf("Error killing process group: %v\n", err)
        }

        wg.Wait()
        logger.Println("Shutdown complete")
    }

    // Connect to Python server
    client, err := services.PythonServer()
    if err != nil {
        logger.Printf("Error connecting to Python server: %v\n", err)
        shutdownServer()
        return
    }

    // Set up scanner
    scanner := bufio.NewScanner(os.Stdin)
    scanner.Split(rpc.Split)

    state := analysis.NewState(client)
    writer := os.Stdout

    // Start signal handler
    go func() {
        <-sigChan
        logger.Println("Received termination signal")
        shutdownServer()
        os.Exit(0)
    }()

    // Main processing loop
    for scanner.Scan() {
        msg := scanner.Bytes()
        method, contents, err := rpc.DecodeMessage(msg)
        if err != nil {
            logger.Printf("Got an error: %s", err)
            continue
        }

        if method == "shutdown" {
            logger.Println("Got a shutdown message")
            shutdownServer()
            break
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

        diagnosticsNotification := state.LintDiagnostics(request.Params.TextDocument.URI)
        writeResponse(writer, diagnosticsNotification)
    case "textDocument/didSave":
        logger.Print("textDocument/didSave")
        var request lsp.DidSaveTextDocumentNotification
        if err := json.Unmarshal(contents, &request); err != nil {
            logger.Printf("textDocument/didSave: %s", err)
            return
        }

        logger.Printf("Saved: %s", request.Params.TextDocument.URI)
        state.SaveDocument(request.Params.TextDocument.URI)

        diagnosticsNotification := state.LintDiagnostics(request.Params.TextDocument.URI)
        writeResponse(writer, diagnosticsNotification)
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
