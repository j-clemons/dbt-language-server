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

func TestParseConfigBlock(t *testing.T) {
    input := `{{ config(materialized='table', unique_key='id') }}
select * from users`

    expected := []Token{
        {Type: DB_LBRACE, Literal: "{{", Line: 0, Column: 0},
        {Type: CONFIG, Literal: "config", Line: 0, Column: 3},
        {Type: LPAREN, Literal: "(", Line: 0, Column: 9},
        {Type: IDENT, Literal: "materialized", Line: 0, Column: 10},
        {Type: EQUAL, Literal: "=", Line: 0, Column: 22},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 0, Column: 23},
        {Type: TABLE, Literal: "table", Line: 0, Column: 24},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 0, Column: 29}, {Type: COMMA, Literal: ",", Line: 0, Column: 30},
        {Type: IDENT, Literal: "unique_key", Line: 0, Column: 32},
        {Type: EQUAL, Literal: "=", Line: 0, Column: 42},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 0, Column: 43},
        {Type: IDENT, Literal: "id", Line: 0, Column: 44},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 0, Column: 46},
        {Type: RPAREN, Literal: ")", Line: 0, Column: 47},
        {Type: DB_RBRACE, Literal: "}}", Line: 0, Column: 49},
        {Type: SELECT, Literal: "select", Line: 1, Column: 0},
        {Type: ASTERISK, Literal: "*", Line: 1, Column: 7},
        {Type: FROM, Literal: "from", Line: 1, Column: 9},
        {Type: IDENT, Literal: "users", Line: 1, Column: 14},
    }

    p := Parse(input, docs.Dialect("snowflake"))
    tokens := p.tokens

    for i, expToken := range expected {
        if expToken != tokens[i].Token {
            t.Fatalf("tokens[%d] - expected=%v, got=%v",
                i, expToken, tokens[i].Token)
        }
    }
}

func TestParseComplexConfigBlock(t *testing.T) {
    input := `{{ config(
    materialized='incremental',
    unique_key='id',
    on_schema_change='fail'
) }}
select * from users`

    p := Parse(input, docs.Dialect("snowflake"))
    tokens := p.tokens

    // Verify that config is properly recognized
    configFound := false
    for _, token := range tokens {
        if token.Token.Type == CONFIG && token.Token.Literal == "config" {
            configFound = true
            break
        }
    }

    if !configFound {
        t.Fatal("CONFIG token not found in parsed tokens")
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
{{ source("source_name", "table_name") }}
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

        {Type: DB_LBRACE, Literal: "{{", Line: 4, Column: 0},
        {Type: SOURCE, Literal: "source", Line: 4, Column: 3},
        {Type: LPAREN, Literal: "(", Line: 4, Column: 9},
        {Type: DOUBLE_QUOTE, Literal: "\"", Line: 4, Column: 10},
        {Type: SOURCE, Literal: "source_name", Line: 4, Column: 11},
        {Type: DOUBLE_QUOTE, Literal: "\"", Line: 4, Column: 22},
        {Type: COMMA, Literal: ",", Line: 4, Column: 23},
        {Type: DOUBLE_QUOTE, Literal: "\"", Line: 4, Column: 25},
        {Type: SOURCE_TABLE, Literal: "table_name", Line: 4, Column: 26},
        {Type: DOUBLE_QUOTE, Literal: "\"", Line: 4, Column: 36},
        {Type: RPAREN, Literal: ")", Line: 4, Column: 37},
        {Type: DB_RBRACE, Literal: "}}", Line: 4, Column: 39},

        {Type: FROM, Literal: "from", Line: 5, Column: 0},
        {Type: DB_LBRACE, Literal: "{{", Line: 5, Column: 5},
        {Type: REF, Literal: "ref", Line: 5, Column: 8},
        {Type: LPAREN, Literal: "(", Line: 5, Column: 11},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 5, Column: 12},
        {Type: REF, Literal: "users", Line: 5, Column: 13},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 5, Column: 18},
        {Type: RPAREN, Literal: ")", Line: 5, Column: 19},
        {Type: DB_RBRACE, Literal: "}}", Line: 5, Column: 21},
    }

    p := Parse(input, docs.Dialect("snowflake"))
    tokens := p.tokens

    for i, expToken := range expected {
        if expToken != tokens[i].Token {
            t.Fatalf("tokens[%d] - expected=%v, got=%v",
                i, expToken, tokens[i])
        }
    }

}

func TestParseJinjaStatementBlocks(t *testing.T) {
    input := `{% set v = var('variable_name') %}
select * from {{ ref('users') }}`

    expected := []Token{
        {Type: JINJA_LBRACE, Literal: "{%", Line: 0, Column: 0},
        {Type: SET, Literal: "set", Line: 0, Column: 3},
        {Type: IDENT, Literal: "v", Line: 0, Column: 7},
        {Type: EQUAL, Literal: "=", Line: 0, Column: 9},
        {Type: VAR, Literal: "var", Line: 0, Column: 11},
        {Type: LPAREN, Literal: "(", Line: 0, Column: 14},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 0, Column: 15},
        {Type: VAR, Literal: "variable_name", Line: 0, Column: 16},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 0, Column: 29},
        {Type: RPAREN, Literal: ")", Line: 0, Column: 30},
        {Type: JINJA_RBRACE, Literal: "%}", Line: 0, Column: 32},

        {Type: SELECT, Literal: "select", Line: 1, Column: 0},
        {Type: ASTERISK, Literal: "*", Line: 1, Column: 7},
        {Type: FROM, Literal: "from", Line: 1, Column: 9},
        {Type: DB_LBRACE, Literal: "{{", Line: 1, Column: 14},
        {Type: REF, Literal: "ref", Line: 1, Column: 17},
        {Type: LPAREN, Literal: "(", Line: 1, Column: 20},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 1, Column: 21},
        {Type: REF, Literal: "users", Line: 1, Column: 22},
        {Type: SINGLE_QUOTE, Literal: "'", Line: 1, Column: 27},
        {Type: RPAREN, Literal: ")", Line: 1, Column: 28},
        {Type: DB_RBRACE, Literal: "}}", Line: 1, Column: 30},
    }

    p := Parse(input, docs.Dialect("snowflake"))
    tokens := p.tokens

    for i, expToken := range expected {
        if i >= len(tokens) {
            t.Fatalf("tokens[%d] - expected=%v, got=<missing>", i, expToken)
        }
        if expToken != tokens[i].Token {
            t.Fatalf("tokens[%d] - expected=%v, got=%v", i, expToken, tokens[i].Token)
        }
    }
}
