package lsp

type DidSaveTextDocumentNotification struct {
	Notification
	Params DidSaveTextDocumentParams `json:"params"`
}

type DidSaveTextDocumentParams struct {
	TextDocument TextDocumentIdentifier `json:"textDocument"`
}
