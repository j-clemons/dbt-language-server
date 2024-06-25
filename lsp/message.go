package lsp

type Request struct {
    RPC    string `json:"jsonrpc"`
    ID     int    `json:"id"`
    Method string `json:"method"`

    // Specify params for all request types
    // Params
}

type Response struct {
    RPC string `json:"jsonrpc"`
    ID  *int   `json:"id"`

    // Result
    // Error
}

type Notification struct {
    RPC    string `json:"jsonrpc"`
    Method string `json:"method"`
}
