package lsp

import "github.com/j-clemons/dbt-language-server/version"

type InitializeRequest struct {
	Request
	Params InitializeRequestParams `json:"params"`
}

type InitializeRequestParams struct {
	ClientInfo ClientInfo `json:"clientInfo"`
	RootPath   string     `json:"rootPath"`
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

	HoverProvider          bool                  `json:"hoverProvider"`
	DefinitionProvider     bool                  `json:"definitionProvider"`
	CompletionProvider     map[string]any        `json:"completionProvider"`
	ExecuteCommandProvider ExecuteCommandOptions `json:"executeCommandProvider"`
}

type ExecuteCommandOptions struct {
	Commands []string `json:"commands"`
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
				TextDocumentSync:   2,
				HoverProvider:      true,
				DefinitionProvider: true,
				CompletionProvider: map[string]any{},
				ExecuteCommandProvider: ExecuteCommandOptions{
					Commands: []string{"dbt.goToSchema"},
				},
			},
			ServerInfo: ServerInfo{
				Name:    "dbt-language-server",
				Version: version.Version,
			},
		},
	}
}
