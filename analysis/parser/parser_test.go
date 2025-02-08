package parser

import (
	"testing"

	"github.com/j-clemons/dbt-language-server/docs"
)

func TestParseCommonTableExpressions(t *testing.T) {
    input := `with cte1 as (
    select *
    from {{ ref('users') }}
),

cte2 as (
    select
    *,
    add(1, 2) as result
    from cte1
)
select *
from cte2`

    p := Parse(input, docs.Dialect("snowflake"))
    ctes := p.ctes.Tokens

    expected := []Token{
        {Type: IDENT, Literal: "cte1", Line: 0, Column: 5},
        {Type: IDENT, Literal: "cte2", Line: 5, Column: 0},
    }

    for i, expCte := range expected {
        if expCte != ctes[i] {
            t.Fatalf("ctes[%d] - expected=%v, got=%v",
                i, expCte, ctes[i])
        }
    }
}

func TestTokenNameMap(t *testing.T) {
    input := `with cte1 as (
    select *
    from {{ ref('users') }}
),

cte2 as (
    select
    *,
    add(1, 2) as result
    from cte1
)
select *
from cte2`

    p := Parse(input, docs.Dialect("snowflake"))
    tokenNameMap := p.CreateTokenNameMap()

    expected := map[string]Token{
        "cte1": {Type: IDENT, Literal: "cte1", Line: 0, Column: 5},
        "cte2": {Type: IDENT, Literal: "cte2", Line: 5, Column: 0},
    }

    for k, v := range expected {
        if v != tokenNameMap[k] {
            t.Fatalf("tokenNameMap[%s] - expected=%v, got=%v",
                k, v, tokenNameMap[k])
        }
    }
}

func TestParseTokens(t *testing.T) {
    input := `select *
{{ var("my_var") }}
{{ ex_macro(input) }}
{{ package.my_macro(input) }}
from {{ ref('users') }}`

    expected := []Token{
        {Type: SELECT, Literal: "select", Line: 0, Column: 0},
        {Type: ASTERISK, Literal: "*", Line: 0, Column: 7},

        {Type: DB_LBRACE, Literal: "{{", Line: 1, Column: 0},
        {Type: VAR, Literal: "var", Line: 1, Column: 3},
        {Type: LPAREN, Literal: "(", Line: 1, Column: 6},
        {Type: DOUBLE_QUOTE, Literal: "\"", Line: 1, Column: 7},
        {Type: VAR, Literal: "my_var", Line: 1, Column: 8},
        {Type: DOUBLE_QUOTE, Literal: "\"", Line: 1, Column: 14},
        {Type: RPAREN, Literal: ")", Line: 1, Column: 15},
        {Type: DB_RBRACE, Literal: "}}", Line: 1, Column: 17},

        {Type: DB_LBRACE, Literal: "{{", Line: 2, Column: 0},
        {Type: MACRO, Literal: "ex_macro", Line: 2, Column: 3},
        {Type: LPAREN, Literal: "(", Line: 2, Column: 11},
        {Type: IDENT, Literal: "input", Line: 2, Column: 12},
        {Type: RPAREN, Literal: ")", Line: 2, Column: 17},
        {Type: DB_RBRACE, Literal: "}}", Line: 2, Column: 19},

        {Type: DB_LBRACE, Literal: "{{", Line: 3, Column: 0},
        {Type: PACKAGE, Literal: "package", Line: 3, Column: 3},
        {Type: DOT, Literal: ".", Line: 3, Column: 10},
        {Type: MACRO, Literal: "my_macro", Line: 3, Column: 11},
        {Type: LPAREN, Literal: "(", Line: 3, Column: 19},
        {Type: IDENT, Literal: "input", Line: 3, Column: 20},
        {Type: RPAREN, Literal: ")", Line: 3, Column: 25},
        {Type: DB_RBRACE, Literal: "}}", Line: 3, Column: 27},

        {Type: FROM, Literal: "from", Line: 4, Column: 0},
        {Type: DB_LBRACE, Literal: "{{", Line: 4, Column: 5},
        {Type: REF, Literal: "ref", Line: 4, Column: 8},
        {Type: LPAREN, Literal: "(", Line: 4, Column: 11},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 4, Column: 12},
        {Type: REF, Literal: "users", Line: 4, Column: 13},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 4, Column: 18},
        {Type: RPAREN, Literal: ")", Line: 4, Column: 19},
        {Type: DB_RBRACE, Literal: "}}", Line: 4, Column: 21},
    }

    p := Parse(input, docs.Dialect("snowflake"))
    tokens := p.tokens

    for i, expToken := range expected {
        if expToken != tokens[i] {
            t.Fatalf("tokens[%d] - expected=%v, got=%v",
                i, expToken, tokens[i])
        }
    }

}
