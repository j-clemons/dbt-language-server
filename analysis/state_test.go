package analysis

import (
	"path/filepath"
	"reflect"
	"testing"

	"github.com/j-clemons/dbt-language-server/docs"
	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/testutils"
)

func expectedTestState() State {
    testdataRoot, err := testutils.GetTestdataPath("jaffle_shop_duckdb")
    if err != nil {
        panic(err)
    }

    expectedState :=  State{
        Documents:map[string]Document{},
        DbtContext: DbtContext{
            ProjectRoot: testdataRoot,
            ProjectYaml: DbtProjectYaml{
                ProjectName:AnnotatedField[string]{
                    Value:"jaffle_shop",
                    Position:lsp.Position{
                        Line:0,
                        Character:6,
                    },
                },
                Profile:AnnotatedField[string]{
                    Value:"jaffle_shop",
                    Position:lsp.Position{
                        Line:5,
                        Character:9,
                    },
                },
                ModelPaths:AnnotatedField[[]string]{
                    Value:[]string{"models"},
                    Position:lsp.Position{
                        Line:7,
                        Character:13,
                    },
                },
                SeedPaths:AnnotatedField[[]string]{
                    Value:[]string{"seeds"},
                    Position:lsp.Position{
                        Line:8,
                        Character:12,
                    },
                },
                MacroPaths:AnnotatedField[[]string]{
                    Value:[]string{"macros"},
                    Position:lsp.Position{
                        Line:11,
                        Character:13,
                    },
                },
                PackagesInstallPath:AnnotatedField[string]{
                    Value:"dbt_packages",
                    Position:lsp.Position{
                        Line:0,
                        Character:0,
                    },
                },
                DocsPaths:AnnotatedField[[]string]{
                    Value:[]string{"models","macros"},
                    Position:lsp.Position{
                        Line:0,
                        Character:0,
                    },
                },
                Vars:AnnotatedMap{
                    "global_count":AnnotatedField[interface{}]{
                        Value:0,
                        Position:lsp.Position{
                            Line:36,
                            Character:16,
                        },
                    },
                    "jaffle_shop":AnnotatedField[interface{}]{
                        Value:AnnotatedMap{
                            "jaffle_number":AnnotatedField[interface{}]{
                                Value:1,
                                Position:lsp.Position{
                                    Line:40,
                                    Character:19,
                                },
                            },
                            "jaffle_string":AnnotatedField[interface{}]{
                                Value:"jaffle",
                                Position:lsp.Position{
                                    Line:39,
                                    Character:19,
                                },
                            },
                        },
                        Position:lsp.Position{
                            Line:39,
                            Character:4,
                        },
                    },
                },
            },
            Dialect: docs.Dialect("duckdb"),
            ModelDetailMap:map[string] ModelDetails{
                "customers": {
                    URI:filepath.Join(testdataRoot, "models/customers.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"This table has basic information about a customer, as well as some derived facts based on a customer's orders",
                },
                "orders": {
                    URI:filepath.Join(testdataRoot, "models/orders.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"This table has basic information about orders, as well as some derived facts based on payments",
                },
                "stg_customer_status": {
                    URI:filepath.Join(testdataRoot, "dbt_packages/jaffle_package/models/stg_customer_status.sql"),
                    ProjectName:"jaffle_package",
                    Description:"",
                },
                "stg_customers": {
                    URI:filepath.Join(testdataRoot, "models/staging/stg_customers.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"",
                },
                "stg_orders": {
                    URI:filepath.Join(testdataRoot, "models/staging/stg_orders.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"",
                },
                "stg_payments": {
                    URI:filepath.Join(testdataRoot, "models/staging/stg_payments.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"",
                },
                "raw_customers": {
                    URI:filepath.Join(testdataRoot, "seeds/raw_customers.csv"),
                    ProjectName:"jaffle_shop",
                    Description:"Seed File",
                },
                "raw_orders": {
                    URI:filepath.Join(testdataRoot, "seeds/raw_orders.csv"),
                    ProjectName:"jaffle_shop",
                    Description:"Seed File",
                },
                "raw_payments": {
                    URI:filepath.Join(testdataRoot, "seeds/raw_payments.csv"),
                    ProjectName:"jaffle_shop",
                    Description:"Seed File",
                },
            },
            MacroDetailMap:map[Package]map[string]Macro{
                "jaffle_package":{
                    "add_values": {
                        Name:"add_values",
                        ProjectName:"jaffle_package",
                        Description:"add_values(arg1, arg2)",
                        URI:filepath.Join(testdataRoot, "dbt_packages/jaffle_package/macros/jaffle_package_macros.sql"),
                        Range:lsp.Range{
                            Start:lsp.Position{
                                Line:0,
                                Character:9,
                            },
                            End:lsp.Position{
                                Line:0,
                                Character:31,
                            },
                        },
                    },
                },
                "jaffle_shop":{
                    "full_name": {
                        Name:"full_name",
                        ProjectName:"jaffle_shop",
                        Description:"full_name(first_name, last_name)",
                        URI:filepath.Join(testdataRoot, "macros/jaffle_macros.sql"),
                        Range:lsp.Range{
                            Start:lsp.Position{
                                Line:0,
                                Character:9,
                            },
                            End:lsp.Position{
                                Line:0,
                                Character:41,
                            },
                        },
                    },
                    "times_five": {
                        Name:"times_five",
                        ProjectName:"jaffle_shop",
                        Description:"times_five(int_value)",
                        URI:filepath.Join(testdataRoot, "macros/jaffle_macros.sql"),
                        Range:lsp.Range{
                            Start:lsp.Position{
                                Line:6,
                                Character:10,
                            },
                            End:lsp.Position{
                                Line:6,
                                Character:31,
                            },
                        },
                    },
                },
            },
            VariableDetailMap: map[string]Variable{
                "global_count": {
                    Name:"global_count",
                    Value:0,
                    URI:filepath.Join(testdataRoot, "dbt_project.yml"),
                    Range:lsp.Range{
                        Start:lsp.Position{
                            Line:36,
                            Character:16,
                        },
                        End:lsp.Position{
                            Line:36,
                            Character:16,
                        },
                    },
                },
                "jaffle_number": {
                    Name:"jaffle_number",
                    Value:1,
                    URI:filepath.Join(testdataRoot, "dbt_project.yml"),
                    Range:lsp.Range{
                        Start:lsp.Position{
                            Line:40,
                            Character:19,
                        },
                        End:lsp.Position{
                            Line:40,
                            Character:19,
                        },
                    },
                },
                "jaffle_string": {
                    Name:"jaffle_string",
                    Value:"jaffle",
                    URI:filepath.Join(testdataRoot, "dbt_project.yml"),
                    Range:lsp.Range{
                        Start:lsp.Position{
                            Line:39,
                            Character:19,
                        },
                        End:lsp.Position{
                            Line:39,
                            Character:19,
                        },
                    },
                },
            },
        },
    }

    return expectedState
}

func TestRefreshDbtContext(t *testing.T) {
    testdataRoot, err := testutils.GetTestdataPath("jaffle_shop_duckdb")
    if err != nil {
        t.Fatal(err)
    }

    expectedState := expectedTestState()

    state := NewState()
    state.refreshDbtContext(testdataRoot)

    if !reflect.DeepEqual(state, expectedState) {
        t.Fatalf("expected %#v,\n\ngot %#v", expectedState, state)
    }
}

func BenchmarkRefreshDbtContext(b *testing.B) {
    for i := 0; i < b.N; i++ {
        testdataRoot, err := testutils.GetTestdataPath("jaffle_shop_duckdb")
        if err != nil {
            b.Fatal(err)
        }

        state := NewState()
        state.refreshDbtContext(testdataRoot)
    }
}
