package parser

import (
	"errors"
	"sort"
)


func tokenize(input string) []Token {
    l := New(input)
    var tokens []Token
    for tok := l.NextToken(); tok.Type != EOF; tok = l.NextToken() {
        tokens = append(tokens, tok)
    }
    return tokens
}

type TokenIndex struct {
    lineTokens map[int][]Token
}

func createTokenIndex(tokens []Token) *TokenIndex {
    index := &TokenIndex{
        lineTokens: make(map[int][]Token),
    }
    
    for _, token := range tokens {
        index.lineTokens[token.Line] = append(index.lineTokens[token.Line], token)
    }
    
    return index
}

func createTokenNameMap(tokens []Token) map[string]Token {
    tokenMap := make(map[string]Token)
    for _, token := range tokens {
        tokenMap[token.Literal] = token
    }
    return tokenMap
}

func Tokenizer(input string) *TokenIndex {
    tokens := tokenize(input)
    return createTokenIndex(tokens)
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
