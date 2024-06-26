package lsp

type TextDocumentDidChangeNotification struct {
    Notification
    Params DidChangeTextDocumentParams `json:"params"`
}

type DidChangeTextDocumentParams struct {
    TextDocument   VersionedTextDocumentIdentifier  `json:"textDocument"`
    ContentChanges []TextDocumentContentChangeEvent `json:"contentChanges"`
}

type TextDocumentContentChangeEvent struct {
    Range       Range  `json:"range"`
    RangeLength int    `json:"rangeLength"`
    Text        string `json:"text"`
}
