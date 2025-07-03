package parser

import "github.com/j-clemons/dbt-language-server/docs"

type TokenType string

type Token struct {
    Type     TokenType
    Literal  string
    Line     int
    Column   int
}

const (
   ILLEGAL           = "ILLEGAL"
   EOF               = "EOF"

   IDENT             = "IDENT"
   INT               = "INT"

   SINGLE_QUOTE      = "'"
   DOUBLE_QUOTE      = "\""
   BACKTICK          = "`"

   EQUAL             = "="
   PLUS              = "+"
   MINUS             = "-"
   BANG              = "!"
   ASTERISK          = "*"
   SLASH             = "/"
   PERCENT           = "%"

   LT                = "<"
   GT                = ">"
   NOT_EQ            = "!="
   LT_EQ             = "<="
   GT_EQ             = ">="

   COMMA             = ","
   SEMICOLON         = ";"
   DOT               = "."

   LPAREN            = "("
   RPAREN            = ")"
   LBRACE            = "{"
   RBRACE            = "}"

   // dbt specfic tokens
   DB_LBRACE         = "{{"
   DB_RBRACE         = "}}"

   REF               = "REF"
   VAR               = "VAR"
   SOURCE            = "SOURCE"
   SOURCE_TABLE      = "SOURCE_TABLE"
   MACRO             = "MACRO"
   PACKAGE           = "PACKAGE"
   CONFIG            = "CONFIG"

   //                keywords
   ACCOUNT           = "ACCOUNT"
   ALL               = "ALL"
   ALTER             = "ALTER"
   ANALYSE           = "ANALYSE"
   ANALYZE           = "ANALYZE"
   AND               = "AND"
   ANY               = "ANY"
   ARRAY             = "ARRAY"
   AS                = "AS"
   ASC               = "ASC"
   ASYMMETRIC        = "ASYMMETRIC"
   BETWEEN           = "BETWEEN"
   BOTH              = "BOTH"
   BY                = "BY"
   CASE              = "CASE"
   CAST              = "CAST"
   CHECK             = "CHECK"
   COLLATE           = "COLLATE"
   COLUMN            = "COLUMN"
   CONNECT           = "CONNECT"
   CONNECTION        = "CONNECTION"
   CONSTRAINT        = "CONSTRAINT"
   CREATE            = "CREATE"
   CROSS             = "CROSS"
   CURRENT           = "CURRENT"
   CURRENT_DATE      = "CURRENT_DATE"
   CURRENT_TIME      = "CURRENT_TIME"
   CURRENT_TIMESTAMP = "CURRENT_TIMESTAMP"
   CURRENT_USER      = "CURRENT_USER"
   DATABASE          = "DATABASE"
   DEFAULT           = "DEFAULT"
   DEFERRABLE        = "DEFERRABLE"
   DELETE            = "DELETE"
   DESC              = "DESC"
   DESCRIBE          = "DESCRIBE"
   DISTINCT          = "DISTINCT"
   DO                = "DO"
   DROP              = "DROP"
   ELSE              = "ELSE"
   END               = "END"
   EXCEPT            = "EXCEPT"
   EXISTS            = "EXISTS"
   FALSE             = "FALSE"
   FETCH             = "FETCH"
   FOLLOWING         = "FOLLOWING"
   FOR               = "FOR"
   FOREIGN           = "FOREIGN"
   FROM              = "FROM"
   FULL              = "FULL"
   GRANT             = "GRANT"
   GROUP             = "GROUP"
   GSCLUSTER         = "GSCLUSTER"
   HAVING            = "HAVING"
   ILIKE             = "ILIKE"
   IN                = "IN"
   INCREMENT         = "INCREMENT"
   INITIALLY         = "INITIALLY"
   INNER             = "INNER"
   INSERT            = "INSERT"
   INTERSECT         = "INTERSECT"
   INTO              = "INTO"
   IS                = "IS"
   ISSUE             = "ISSUE"
   JOIN              = "JOIN"
   LATERAL           = "LATERAL"
   LEADING           = "LEADING"
   LEFT              = "LEFT"
   LIKE              = "LIKE"
   LIMIT             = "LIMIT"
   LOCALTIME         = "LOCALTIME"
   LOCALTIMESTAMP    = "LOCALTIMESTAMP"
   MINUS_KW          = "MINUS"
   NATURAL           = "NATURAL"
   NOT               = "NOT"
   NULL              = "NULL"
   OF                = "OF"
   OFFSET            = "OFFSET"
   ON                = "ON"
   ONLY              = "ONLY"
   OR                = "OR"
   ORDER             = "ORDER"
   ORGANIZATION      = "ORGANIZATION"
   PIVOT             = "PIVOT"
   PIVOT_LONGER      = "PIVOT_LONGER"
   PIVOT_WIDER       = "PIVOT_WIDER"
   PLACING           = "PLACING"
   PRIMARY           = "PRIMARY"
   QUALIFY           = "QUALIFY"
   RECURSIVE         = "RECURSIVE"
   RECURSIVE_LONGER  = "RECURSIVE_LONGER"
   RECURSIVE_WIDER   = "RECURSIVE_WIDER"
   REFERENCES        = "REFERENCES"
   REGEXP            = "REGEXP"
   REJECT            = "REJECT"
   RETURNING         = "RETURNING"
   RETURNING_LONGER  = "RETURNING_LONGER"
   REVOKE            = "REVOKE"
   RIGHT             = "RIGHT"
   RLIKE             = "RLIKE"
   ROW               = "ROW"
   ROWS              = "ROWS"
   SAMPLE            = "SAMPLE"
   SCHEMA            = "SCHEMA"
   SELECT            = "SELECT"
   SET               = "SET"
   SHOW              = "SHOW"
   SOME              = "SOME"
   START             = "START"
   SUMMARIZE         = "SUMMARIZE"
   SYMMETRIC         = "SYMMETRIC"
   TABLE             = "TABLE"
   TABLESAMPLE       = "TABLESAMPLE"
   THEN              = "THEN"
   TO                = "TO"
   TRAILING          = "TRAILING"
   TRIGGER           = "TRIGGER"
   TRUE              = "TRUE"
   TRY_CAST          = "TRY_CAST"
   UNION             = "UNION"
   UNIQUE            = "UNIQUE"
   UNPIVOT           = "UNPIVOT"
   UPDATE            = "UPDATE"
   USING             = "USING"
   VALUES            = "VALUES"
   VARIADIC          = "VARIADIC"
   VIEW              = "VIEW"
   WHEN              = "WHEN"
   WHENEVER          = "WHENEVER"
   WHERE             = "WHERE"
   WINDOW            = "WINDOW"
   WITH              = "WITH"
)

var snowflakeKeywords = map[string]TokenType{
    "account":           ACCOUNT,
    "all":               ALL,
    "alter":             ALTER,
    "and":               AND,
    "any":               ANY,
    "as":                AS,
    "between":           BETWEEN,
    "by":                BY,
    "case":              CASE,
    "cast":              CAST,
    "check":             CHECK,
    "column":            COLUMN,
    "connect":           CONNECT,
    "connection":        CONNECTION,
    "constraint":        CONSTRAINT,
    "create":            CREATE,
    "cross":             CROSS,
    "current":           CURRENT,
    "current_date":      CURRENT_DATE,
    "current_time":      CURRENT_TIME,
    "current_timestamp": CURRENT_TIMESTAMP,
    "current_user":      CURRENT_USER,
    "database":          DATABASE,
    "delete":            DELETE,
    "distinct":          DISTINCT,
    "drop":              DROP,
    "else":              ELSE,
    "exists":            EXISTS,
    "false":             FALSE,
    "following":         FOLLOWING,
    "for":               FOR,
    "from":              FROM,
    "full":              FULL,
    "grant":             GRANT,
    "group":             GROUP,
    "gscluster":         GSCLUSTER,
    "having":            HAVING,
    "ilike":             ILIKE,
    "in":                IN,
    "increment":         INCREMENT,
    "inner":             INNER,
    "insert":            INSERT,
    "intersect":         INTERSECT,
    "into":              INTO,
    "is":                IS,
    "issue":             ISSUE,
    "join":              JOIN,
    "lateral":           LATERAL,
    "left":              LEFT,
    "like":              LIKE,
    "localtime":         LOCALTIME,
    "localtimestamp":    LOCALTIMESTAMP,
    "minus":             MINUS_KW,
    "natural":           NATURAL,
    "not":               NOT,
    "null":              NULL,
    "of":                OF,
    "on":                ON,
    "or":                OR,
    "order":             ORDER,
    "organization":      ORGANIZATION,
    "qualify":           QUALIFY,
    "regexp":            REGEXP,
    "revoke":            REVOKE,
    "right":             RIGHT,
    "rlike":             RLIKE,
    "row":               ROW,
    "rows":              ROWS,
    "sample":            SAMPLE,
    "schema":            SCHEMA,
    "select":            SELECT,
    "set":               SET,
    "some":              SOME,
    "start":             START,
    "table":             TABLE,
    "tablesample":       TABLESAMPLE,
    "then":              THEN,
    "to":                TO,
    "trigger":           TRIGGER,
    "true":              TRUE,
    "try_cast":          TRY_CAST,
    "union":             UNION,
    "unique":            UNIQUE,
    "update":            UPDATE,
    "using":             USING,
    "values":            VALUES,
    "view":              VIEW,
    "when":              WHEN,
    "whenever":          WHENEVER,
    "where":             WHERE,
    "with":              WITH,
}

var duckdbKeywords = map[string]TokenType{
    "all":          ALL,
    "analyse":      ANALYSE,
    "analyze":      ANALYZE,
    "and":          AND,
    "any":          ANY,
    "array":        ARRAY,
    "as":           AS,
    "asc":          ASC,
    "asymmetric":   ASYMMETRIC,
    "both":         BOTH,
    "case":         CASE,
    "cast":         CAST,
    "check":        CHECK,
    "collate":      COLLATE,
    "column":       COLUMN,
    "constraint":   CONSTRAINT,
    "create":       CREATE,
    "default":      DEFAULT,
    "deferrable":   DEFERRABLE,
    "desc":         DESC,
    "describe":     DESCRIBE,
    "distinct":     DISTINCT,
    "do":           DO,
    "else":         ELSE,
    "end":          END,
    "except":       EXCEPT,
    "false":        FALSE,
    "fetch":        FETCH,
    "for":          FOR,
    "foreign":      FOREIGN,
    "from":         FROM,
    "grant":        GRANT,
    "group":        GROUP,
    "having":       HAVING,
    "in":           IN,
    "initially":    INITIALLY,
    "intersect":    INTERSECT,
    "into":         INTO,
    "lateral":      LATERAL,
    "leading":      LEADING,
    "limit":        LIMIT,
    "not":          NOT,
    "null":         NULL,
    "offset":       OFFSET,
    "on":           ON,
    "only":         ONLY,
    "or":           OR,
    "order":        ORDER,
    "pivot":        PIVOT,
    "pivot_longer": PIVOT_LONGER,
    "pivot_wider":  PIVOT_WIDER,
    "placing":      PLACING,
    "primary":      PRIMARY,
    "qualify":      QUALIFY,
    "references":   REFERENCES,
    "returning":    RETURNING,
    "select":       SELECT,
    "show":         SHOW,
    "some":         SOME,
    "summarize":    SUMMARIZE,
    "symmetric":    SYMMETRIC,
    "table":        TABLE,
    "then":         THEN,
    "to":           TO,
    "trailing":     TRAILING,
    "true":         TRUE,
    "union":        UNION,
    "unique":       UNIQUE,
    "unpivot":      UNPIVOT,
    "using":        USING,
    "variadic":     VARIADIC,
    "when":         WHEN,
    "where":        WHERE,
    "window":       WINDOW,
    "with":         WITH,
}

func LookupIdent(ident string, dialect docs.Dialect) TokenType {
    keywords := map[string]TokenType{}
    switch dialect {
    case "snowflake":
        keywords = snowflakeKeywords
    case "duckdb":
        keywords = duckdbKeywords
    }

    // dbt keywords
    keywords["ref"] = REF
    keywords["var"] = VAR
    keywords["source"] = SOURCE
    keywords["config"] = CONFIG
    if tok, ok := keywords[ident]; ok {
        return tok
    }
    return IDENT
}
