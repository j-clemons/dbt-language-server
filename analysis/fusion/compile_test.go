package fusion

import (
	"testing"

	"github.com/j-clemons/dbt-language-server/lsp"
)

func TestParseFusionLogMsg(t *testing.T) {
	var tests = []struct {
		input     string
		code      string
		level     string
		act_uri   string
		act_msg   string
		act_range lsp.Range
	}{
		{
			`\u001b[31m\u001b[1merror:\u001b[0m dbt0101: mismatched input '\u001b[33msum\u001b[0m' expecting one of '\u001b[33m,\u001b[0m', '\u001b[33mEXCEPT\u001b[0m', '\u001b[33mFROM\u001b[0m', '\u001b[33mGROUP\u001b[0m', '\u001b[33mHAVING\u001b[0m', '\u001b[33mINTERSECT\u001b[0m' ...\n  --> models/marts/orders.sql:25:9 (target/compiled/models/marts/orders.sql:25:9)`,
			"0101",
			"error",
			"models/marts/orders.sql",
			"mismatched input 'sum' expecting one of ',', 'EXCEPT', 'FROM', 'GROUP', 'HAVING', 'INTERSECT' ...",
			lsp.Range{
				Start: lsp.Position{
					Line:      24,
					Character: 8,
				},
				End: lsp.Position{
					Line:      24,
					Character: 8,
				},
			},
		},
		{
			"error: dbt1502: Failed to render SQL syntax error: unexpected `}`, expected end of variable block\n(in models/marts/orders.sql:11)\n  --> models/marts/orders.sql:11:41",
			"1502",
			"error",
			"models/marts/orders.sql",
			"Failed to render SQL syntax error: unexpected `}`, expected end of variable block",
			lsp.Range{
				Start: lsp.Position{
					Line:      10,
					Character: 40,
				},
				End: lsp.Position{
					Line:      10,
					Character: 40,
				},
			},
		},
	}

	for _, test := range tests {
		act_uri, act_msg, act_range := parseFusionLogMsg(test.input, test.code, test.level)
		if act_uri != test.act_uri {
			t.Errorf("Expected %s, got %s", test.act_uri, act_uri)
		}
		if act_msg != test.act_msg {
			t.Errorf("Expected %s, got %s", test.act_msg, act_msg)
		}
		if act_range != test.act_range {
			t.Errorf("Expected %v, got %v", test.act_range, act_range)
		}
	}
}
