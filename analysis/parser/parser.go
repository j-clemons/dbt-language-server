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
    tokens  []TokenLL
    ctes    CTE
}

type CTE struct {
    Ind          bool
    ParenCount   int
    Tokens       []Token
    TokenNameMap map[string]Token
}

type TokenLL struct {
    Token     Token
    PrevToken *TokenLL
}

func NewParser(input string, dialect docs.Dialect) *Parser {
    return &Parser{
        l: New(input, dialect),
        ctes: CTE{
            Ind:        false,
            ParenCount: -1,
            Tokens:     []Token{},
        },
    }
}

func Parse(input string, dialect docs.Dialect) *Parser {
    p := NewParser(input, dialect)
    p.parseTokens()
    return p
}

func (p *Parser) NextToken() Token {
    if p.curTok.Type != "" {
        prevToken := (*TokenLL)(nil)
        if len(p.tokens) > 0 {
            prevToken = &p.tokens[len(p.tokens) - 1]
        }
        p.tokens = append(p.tokens, TokenLL{
            Token:     p.curTok,
            PrevToken: prevToken,
        })
    }

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

func (p *Parser) parseRef() {
    p.NextToken()
    if p.curTok.Type == LPAREN {
        p.incParenCount()
        p.NextToken()
        if p.curTok.Type == SINGLE_QUOTE || p.curTok.Type == DOUBLE_QUOTE {
            p.NextToken()
            if p.curTok.Type == IDENT {
                p.curTok.Type = REF
            }
        }
    }
}

func (p *Parser) parseVar() {
    p.NextToken()
    if p.curTok.Type == LPAREN {
        p.incParenCount()
        p.NextToken()
        if p.curTok.Type == SINGLE_QUOTE || p.curTok.Type == DOUBLE_QUOTE {
            p.NextToken()
            if p.curTok.Type == IDENT {
                p.curTok.Type = VAR
            }
        }
    }
}

func (p *Parser) parseMacro() {
    if p.peekTok.Type == DOT {
        p.curTok.Type = PACKAGE
        p.NextToken()
        p.NextToken()
    }
    if p.curTok.Type == IDENT && p.peekTok.Type == LPAREN {
        p.curTok.Type = MACRO
    }
}

func (p *Parser) parseSource() {
    p.NextToken()
    if p.curTok.Type == LPAREN {
        p.incParenCount()
        p.NextToken()
        if p.curTok.Type == SINGLE_QUOTE || p.curTok.Type == DOUBLE_QUOTE {
            p.NextToken()
            if p.curTok.Type == IDENT {
                p.curTok.Type = SOURCE
                p.NextToken()
                if p.curTok.Type == SINGLE_QUOTE || p.curTok.Type == DOUBLE_QUOTE {
                    p.NextToken()
                    if p.curTok.Type == COMMA {
                        p.NextToken()
                        if p.curTok.Type == SINGLE_QUOTE || p.curTok.Type == DOUBLE_QUOTE {
                            p.NextToken()
                            if p.curTok.Type == IDENT {
                                p.curTok.Type = SOURCE_TABLE
                            }
                        }
                    }
                }
            }
        }
    }
}

func (p *Parser) parseConfig() {
    p.NextToken()
    if p.curTok.Type == LPAREN {
        p.incParenCount()
        // Parse config parameters - we'll mark the config function call
        // and let the rest of the parsing handle the parameters normally
    }
}

func (p *Parser) incParenCount() {
    if p.ctes.Ind {
        p.ctes.ParenCount++
    }
}

func (p *Parser) decParenCount() {
    if p.ctes.Ind {
        p.ctes.ParenCount--
    }
}

func (p *Parser) parseTokens() {
    for p.curTok.Type != EOF {
        switch p.curTok.Type {
        case WITH:
            p.parseWith()
        case LPAREN:
            p.incParenCount()
        case RPAREN:
            p.decParenCount()
            if p.ctes.Ind && p.ctes.ParenCount == 0 {
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
        case SOURCE:
            p.parseSource()
        case REF:
            p.parseRef()
        case VAR:
            p.parseVar()
        case DB_LBRACE:
            switch p.peekTok.Type {
            case CONFIG:
                p.NextToken()
                p.parseConfig()
            case IDENT:
                p.NextToken()
                p.parseMacro()
            }
        case JINJA_LBRACE:
        case DB_RBRACE:
        case JINJA_RBRACE:
        }
        p.NextToken()
    }
}

func (p *Parser) CreateTokenNameMap() map[string]Token {
    tokenMap := make(map[string]Token)
    for _, token := range p.ctes.Tokens {
        tokenMap[token.Literal] = token
    }
    return tokenMap
}

type TokenIndex struct {
    lineTokens map[int][]TokenLL
}

func (p *Parser) CreateTokenIndex() *TokenIndex {
    index := &TokenIndex{
        lineTokens: make(map[int][]TokenLL),
    }

    for _, t := range p.tokens {
        index.lineTokens[t.Token.Line] = append(index.lineTokens[t.Token.Line], t)
    }

    return index
}

func (ti *TokenIndex) FindTokenAtCursor(line, column int) (*TokenLL, error) {
    lineTokens, exists := ti.lineTokens[line]
    if !exists {
        return nil, errors.New("line does not exist")
    }

    // Binary search to find the token
    idx := sort.Search(len(lineTokens), func(i int) bool {
        return lineTokens[i].Token.Column + len(lineTokens[i].Token.Literal) > column
    })

    if idx >= 0 &&
        column >= lineTokens[idx].Token.Column &&
        column < lineTokens[idx].Token.Column + len(lineTokens[idx].Token.Literal) {
        return &lineTokens[idx], nil
    }

    return nil, errors.New("token does not exist")
}
