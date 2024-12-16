package analysis

import (
	"path/filepath"
	"reflect"
	"testing"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/testutils"
)

func expectedTestState() State {
    testdataRoot, err := testutils.GetTestdataPath("jaffle_shop_duckdb")
    if err != nil {
        panic(err)
    }

    expectedState :=  State{
        Documents:map[string]string{},
        DbtContext: DbtContext{
            ProjectRoot:"/home/jclemons/Projects/dbt-lsp/testdata/jaffle_shop_duckdb",
            ProjectYaml: DbtProjectYaml{
                ProjectName:"jaffle_shop",
                ModelPaths:[]string{"models"},
                MacroPaths:[]string{"macros"},
                PackagesInstallPath:"dbt_packages",
                DocsPaths:[]string{"models","macros"},
                Vars:map[string]interface {}{
                    "global_count":0,
                    "jaffle_shop":map[string]interface {}{"jaffle_number":1,"jaffle_string":"jaffle"},
                },
            },
            ModelDetailMap:map[string] ModelDetails{
                "customers": ModelDetails{
                    URI:filepath.Join(testdataRoot, "models/customers.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"This table has basic information about a customer, as well as some derived facts based on a customer's orders",
                },
                "orders": ModelDetails{
                    URI:filepath.Join(testdataRoot, "models/orders.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"This table has basic information about orders, as well as some derived facts based on payments",
                },
                "stg_customer_status": ModelDetails{
                    URI:filepath.Join(testdataRoot, "dbt_packages/jaffle_package/models/stg_customer_status.sql"),
                    ProjectName:"jaffle_package",
                    Description:"",
                },
                "stg_customers": ModelDetails{
                    URI:filepath.Join(testdataRoot, "models/staging/stg_customers.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"",
                },
                "stg_orders": ModelDetails{
                    URI:filepath.Join(testdataRoot, "models/staging/stg_orders.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"",
                },
                "stg_payments": ModelDetails{
                    URI:filepath.Join(testdataRoot, "models/staging/stg_payments.sql"),
                    ProjectName:"jaffle_shop",
                    Description:"",
                },
            },
            MacroDetailMap:map[string] Macro{
                "add_values": Macro{
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
                "full_name": Macro{
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
                "times_five": Macro{
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
            VariableDetailMap: map[string]Variable{
                "global_count": Variable{
                    Name:"global_count",
                    Value:0,
                    URI:filepath.Join(testdataRoot, "dbt_project.yml"),
                    Range:lsp.Range{
                        Start:lsp.Position{
                            Line:36,
                            Character:14,
                        },
                        End:lsp.Position{
                            Line:36,
                            Character:14,
                        },
                    },
                },
                "jaffle_number": Variable{
                    Name:"jaffle_number",
                    Value:1,
                    URI:filepath.Join(testdataRoot, "dbt_project.yml"),
                    Range:lsp.Range{
                        Start:lsp.Position{
                            Line:40,
                            Character:17,
                        },
                        End:lsp.Position{
                            Line:40,
                            Character:17,
                        },
                    },
                },
                "jaffle_string": Variable{
                    Name:"jaffle_string",
                    Value:"jaffle",
                    URI:filepath.Join(testdataRoot, "dbt_project.yml"),
                    Range:lsp.Range{
                        Start:lsp.Position{
                            Line:39,
                            Character:17,
                        },
                        End:lsp.Position{
                            Line:39,
                            Character:17,
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
        t.Fatalf("expected %v, got %v", expectedState, state)
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
