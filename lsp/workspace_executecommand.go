package lsp

type ExecuteCommandRequest struct {
	Request
	Params ExecuteCommandParams `json:"params"`
}

type ExecuteCommandParams struct {
	Command   string `json:"command"`
	Arguments []any  `json:"arguments"`
}

type ExecuteCommandResponse struct {
	Response
	Result any `json:"result"`
}

type GoToSchemaParams struct {
	URI      string   `json:"uri"`
	Position Position `json:"position"`
}
