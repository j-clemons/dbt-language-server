package parser

import (
	"errors"
	"sort"

	"github.com/j-clemons/dbt-language-server/docs"
)


type Parser struct {
    l       *Lexer
    curTok  Token
    peekTok Token
    tokens  []Token
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
    p.tokens = append(p.tokens, p.curTok)

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
    dbtToken := false
    for p.curTok.Type != EOF {
        p.curTok.DbtToken = dbtToken
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
        case DB_LBRACE:
            dbtToken = true
        case DB_RBRACE:
            dbtToken = false
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

type TokenIndex struct {
    lineTokens map[int][]Token
}

func (p *Parser) CreateTokenIndex() *TokenIndex {
    index := &TokenIndex{
        lineTokens: make(map[int][]Token),
    }

    for _, token := range p.tokens {
        index.lineTokens[token.Line] = append(index.lineTokens[token.Line], token)
    }

    return index
}

func (ti *TokenIndex) FindTokenAtCursor(line, column int) (*Token, error) {
    lineTokens, exists := ti.lineTokens[line]
    if !exists {
        return nil, errors.New("line does not exist")
    }

    // Binary search to find the token
    idx := sort.Search(len(lineTokens), func(i int) bool {
        return lineTokens[i].Column + len(lineTokens[i].Literal) > column
    })

    if idx >= 0 &&
       column >= lineTokens[idx].Column &&
       column < lineTokens[idx].Column + len(lineTokens[idx].Literal) {
        return &lineTokens[idx], nil
    }

    return nil, errors.New("token does not exist")
}
