package analysis

import (
	"fmt"
	"path/filepath"
	"testing"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/testutils"
)

func TestParsePropertiesYamlFile(t *testing.T) {
    testdataRoot, err := testutils.GetTestdataPath("jaffle_shop_duckdb")
    if err != nil {
        panic(err)
    }

    actualProperties := parsePropertiesYamlFile(
        filepath.Join(testdataRoot, "models/schema.yml"),
    )

    expectedProperties := PropertiesYaml{
        Models:[]ModelProperties{
            {
                Name:AnnotatedField[string]{
                    Value:"customers", Position:lsp.Position{Line:3, Character:10},
                },
                Description:AnnotatedField[string]{
                    Value:"This table has basic information about a customer, as well as some derived facts based on a customer's orders",
                    Position:lsp.Position{Line:4, Character:17},
                },
                ModelConfig:AnnotatedMap(nil),
            },
            {
                Name:AnnotatedField[string]{
                    Value:"orders",
                    Position:lsp.Position{Line: 31, Character:10},
                },
                Description:AnnotatedField[string]{
                    Value:"This table has basic information about orders, as well as some derived facts based on payments",
                    Position:lsp.Position{Line:32, Character:17},
                },
                ModelConfig:AnnotatedMap(nil),
            },
        },
        Sources:[]SourceProperties{
            {
                Name:AnnotatedField[string]{
                    Value:"jaffle_shop",
                    Position:lsp.Position{Line:84, Character:10},
                },
                Database:AnnotatedField[string]{
                    Value:"raw",
                    Position:lsp.Position{Line:85, Character:14},
                },
                Schema:AnnotatedField[string]{
                    Value:"jaffle_shop",
                    Position:lsp.Position{Line:86, Character:12},
                },
                Description:AnnotatedField[string]{
                    Value:"",
                    Position:lsp.Position{Line:0, Character:0},
                },
                Tables:[]SourceTableProperties{
                    {
                        Name:AnnotatedField[string]{
                            Value:"orders",
                            Position:lsp.Position{Line:88, Character:14},
                        },

                        Description:AnnotatedField[string]{
                            Value:"",
                            Position:lsp.Position{Line:0, Character:0},
                        },
                    },
                    {
                        Name:AnnotatedField[string]{
                            Value:"customers",
                            Position:lsp.Position{Line:89, Character:14},
                        },
                        Description:AnnotatedField[string]{
                            Value:"",
                            Position:lsp.Position{Line:0, Character:0},
                        },
                    },
                },
            },
            {
                Name:AnnotatedField[string]{
                    Value:"stripe",
                    Position:lsp.Position{Line:91, Character:10},
                },
                Database:AnnotatedField[string]{
                    Value:"",
                    Position:lsp.Position{Line:0, Character:0},
                },
                Schema:AnnotatedField[string]{
                    Value:"",
                    Position:lsp.Position{Line:0, Character:0},
                },
                Description:AnnotatedField[string]{
                    Value:"",
                    Position:lsp.Position{Line:0, Character:0},
                },
                Tables:[]SourceTableProperties{
                    {
                        Name:AnnotatedField[string]{
                            Value:"payments",
                            Position:lsp.Position{Line:93, Character:14},
                        },
                        Description:AnnotatedField[string]{
                            Value:"",
                            Position:lsp.Position{Line:0, Character:0},
                        },
                    },
                },
            },
        },
    }

    if fmt.Sprintf("%#v", actualProperties) != fmt.Sprintf("%#v", expectedProperties) {
        t.Errorf("expected %#v but got %#v", expectedProperties, actualProperties)
    }
}
