package util

import (
	"io"

	"github.com/j-clemons/dbt-language-server/rpc"
)

func WriteResponse(writer io.Writer, msg any) {
	reply := rpc.EncodeMessage(msg)
	writer.Write([]byte(reply))
}
