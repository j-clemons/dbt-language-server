package lsp

type DiagnosticsNotification struct {
    Notification
    Params PublishDiagnosticsParams `json:"params"`
}

type PublishDiagnosticsParams struct {
    URI         string       `json:"uri"`
    Diagnostics []Diagnostic `json:"diagnostics"`
}

type Diagnostic struct {
    Range       Range  `json:"range"`
    Message     string `json:"message"`
    Severity    int    `json:"severity"`
    Code        string `json:"code"`
    Source      string `json:"source"`
}
