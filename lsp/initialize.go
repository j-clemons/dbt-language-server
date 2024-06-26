package lsp

type InitializeRequest struct {
    Request
    Params InitializeRequestParams `json:"params"`
}

type InitializeRequestParams struct {
    ClientInfo ClientInfo `json:"clientInfo"`
}

type ClientInfo struct {
    Name    string `json:"name"`
    Version string `json:"version"`
}

type InitializeResponse struct {
    Response
    Result InitializeResult `json:"result"`
}

type InitializeResult struct {
    Capabilities ServerCapabilities `json:"capabilities"`
    ServerInfo   ServerInfo         `json:"serverInfo"`
}

type ServerCapabilities struct {
    TextDocumentSync int `json:"textDocumentSync"`
}

type ServerInfo struct {
    Name    string `json:"name"`
    Version string `json:"version"`
}

func NewInitializeResponse(id int) InitializeResponse {
    return InitializeResponse{
        Response: Response{
            RPC: "2.0",
            ID:  &id,
        },
        Result: InitializeResult{
            Capabilities: ServerCapabilities{
                TextDocumentSync: 1,
            },
            ServerInfo:   ServerInfo{
                Name:    "dbt-language-server",
                Version: "0.0.0.0",
            },
        },
    }
}
