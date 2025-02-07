package parser

import "github.com/j-clemons/dbt-language-server/docs"


type Parser struct {
    l       *Lexer
    curTok  Token
    peekTok Token
    ctes    CTE
}

type CTE struct {
    Ind          bool
    ParenCount   int
    Tokens       []Token
    TokenNameMap map[string]Token
}

func NewParser(input string, dialect docs.Dialect) *Parser {
    return &Parser{
        l: New(input, dialect),
        ctes: CTE{
            Ind: false,
            ParenCount: -1,
            Tokens: []Token{},
        },
    }
}

func Parse(input string, dialect docs.Dialect) *Parser {
    p := NewParser(input, dialect)
    p.parseTokens()
    return p
}

func (p *Parser) NextToken() Token {
    p.curTok = p.peekTok
    p.peekTok = p.l.NextToken()
    return p.curTok
}

func (p *Parser) parseWith() {
    p.NextToken()
    if p.curTok.Type == IDENT {
        p.ctes.Ind = true
        p.ctes.Tokens = append(p.ctes.Tokens, p.curTok)
        if p.peekTok.Type == AS {
            p.NextToken()
        }
        if p.peekTok.Type == LPAREN {
            p.ctes.ParenCount = 1
        }
        p.NextToken()
    }
}

func (p *Parser) parseTokens() []Token {
    for p.curTok.Type != EOF {
        switch p.curTok.Type {
        case WITH:
            p.parseWith()
        case LPAREN:
            if p.ctes.Ind {
                p.ctes.ParenCount++
            }
        case RPAREN:
            if p.ctes.Ind {
                p.ctes.ParenCount--

                if p.ctes.ParenCount == 0 {
                    p.NextToken()
                    if p.curTok.Type == COMMA {
                        p.NextToken()
                        if p.curTok.Type == IDENT {
                            p.ctes.Tokens = append(p.ctes.Tokens, p.curTok)
                        }
                    } else {
                        p.ctes.Ind = false
                    }
                }
            }
        }
        p.NextToken()
    }
    return p.ctes.Tokens
}

func (p *Parser) CreateTokenNameMap() map[string]Token {
    tokenMap := make(map[string]Token)
    for _, token := range p.ctes.Tokens {
        tokenMap[token.Literal] = token
    }
    return tokenMap
}
