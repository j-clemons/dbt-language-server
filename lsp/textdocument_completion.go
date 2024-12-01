package lsp

type CompletionRequest struct {
    Request
    Params CompletionParams `json:"params"`
}

type CompletionParams struct {
    TextDocumentPositionParams
}

type CompletionResponse struct {
    Response
    Result []CompletionItem `json:"result"`
}

type CompletionItem struct {
    Label string `json:"label"`
}

type CompletionOptions struct {
    triggerCharacters []string `json:"triggerCharacters"`
}
