package parser

import "testing"

func TestNextToken(t *testing.T) {
    input := `select *
from {{ ref('users') }}`

    tests := []Token{
        Token{Type: SELECT, Literal: "select", Line: 0, Column: 0},
        Token{Type: ASTERISK, Literal: "*", Line: 0, Column: 7},
        Token{Type: FROM, Literal: "from", Line: 1, Column: 0},
        Token{Type: DB_LBRACE, Literal: "{{", Line: 1, Column: 5},
        Token{Type: REF, Literal: "ref", Line: 1, Column: 8},
        Token{Type: LPAREN, Literal: "(", Line: 1, Column: 11},
        Token{Type: SINGLE_QUOTE, Literal: "'", Line: 1, Column: 12},
        Token{Type: IDENT, Literal: "users", Line: 1, Column: 13},
        Token{Type: SINGLE_QUOTE, Literal: "'", Line: 1, Column: 18},
        Token{Type: RPAREN, Literal: ")", Line: 1, Column: 19},
        Token{Type: DB_RBRACE, Literal: "}}", Line: 1, Column: 21},
    }

    l := New(input)

    for i, tt := range tests {
        tok := l.NextToken()

        if tok != tt {
            t.Fatalf("tests[%d] - expected=%v, got=%v",
                i, tt, tok)
        }
    }
}
