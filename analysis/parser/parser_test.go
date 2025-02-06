package parser

import (
	"testing"

	"github.com/j-clemons/dbt-language-server/docs"
)

func TestCommonTableExpressions(t *testing.T) {
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

    p := NewParser(input, docs.Dialect("snowflake"))
    ctes := p.CommonTableExpressions()

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
