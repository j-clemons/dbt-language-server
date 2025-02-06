package parser

import (
	"testing"

	"github.com/j-clemons/dbt-language-server/docs"
)

func TestNextToken(t *testing.T) {
    input := `select *
from {{ ref('users') }}`

    tests := []Token{
        {Type: SELECT, Literal: "select", Line: 0, Column: 0},
        {Type: ASTERISK, Literal: "*", Line: 0, Column: 7},
        {Type: FROM, Literal: "from", Line: 1, Column: 0},
        {Type: DB_LBRACE, Literal: "{{", Line: 1, Column: 5},
        {Type: REF, Literal: "ref", Line: 1, Column: 8},
        {Type: LPAREN, Literal: "(", Line: 1, Column: 11},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 1, Column: 12},
        {Type: IDENT, Literal: "users", Line: 1, Column: 13},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 1, Column: 18},
        {Type: RPAREN, Literal: ")", Line: 1, Column: 19},
        {Type: DB_RBRACE, Literal: "}}", Line: 1, Column: 21},
    }

    l := New(input, docs.Dialect("snowflake"))

    for i, tt := range tests {
        tok := l.NextToken()

        if tok != tt {
            t.Fatalf("tests[%d] - expected=%v, got=%v",
                i, tt, tok)
        }
    }
}
