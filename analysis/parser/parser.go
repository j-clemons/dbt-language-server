package parser


type Parser struct {
    l       *Lexer
    curTok  Token
    peekTok Token
}

func NewParser(input string) *Parser {
    return &Parser{
        l: New(input),
    }
}

func Parse(input string) map[string]Token {
    p := NewParser(input)
    ctes := p.CommonTableExpressions()
    return createTokenNameMap(ctes)
}

func (p *Parser) NextToken() Token {
    p.curTok = p.peekTok
    p.peekTok = p.l.NextToken()
    return p.curTok
}

func (p *Parser) CommonTableExpressions() []Token {
    var ctes []Token
    for p.curTok.Type != EOF {
        if p.curTok.Type == WITH {
            p.NextToken()
            for p.curTok.Type != EOF {
                if p.curTok.Type == IDENT {
                    ctes = append(ctes, p.curTok)
                    p.NextToken()

                    for (p.curTok.Type != LPAREN && p.curTok.Type != EOF) {
                        p.NextToken()
                    }
                    openParen := 1
                    for (openParen > 0 && p.curTok.Type != EOF) {
                        p.NextToken()
                        if p.curTok.Type == LPAREN {
                            openParen++
                        } else if p.curTok.Type == RPAREN {
                            openParen--
                        }
                    }

                    p.NextToken()
                    if p.curTok.Type != COMMA {
                        return ctes
                    }
                }
                p.NextToken()
            }
        }
        p.NextToken()
    }
    return ctes
}
