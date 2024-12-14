package analysis

import (
	"path/filepath"
	"reflect"
	"testing"

	"github.com/j-clemons/dbt-language-server/testutils"
)

func expectedTestState() State {
    testdataRoot, err := testutils.GetTestdataPath("jaffle_shop_duckdb")
    if err != nil {
        panic(err)
    }
    expectedState := State{
        Documents: map[string]string{},
        DbtContext: DbtContext{
            ProjectRoot: testdataRoot,
            ProjectYaml: DbtProjectYaml{
                ProjectName:         "jaffle_shop",
                ModelPaths:          []string{"models"},
                MacroPaths:          []string{"macros"},
                PackagesInstallPath: "",
                DocsPaths:           []string{"models","macros"},
                Vars:                map[string]interface {}(nil),
            },
            ModelDetailMap: map[string]ModelDetails{
                "customers": ModelDetails{
                    URI:         filepath.Join(testdataRoot, "models/customers.sql"),
                    ProjectName: "jaffle_shop",
                    Description: "This table has basic information about a customer, as well as some derived facts based on a customer's orders",
                },
                "orders": ModelDetails{
                    URI:         filepath.Join(testdataRoot, "models/orders.sql"),
                    ProjectName: "jaffle_shop",
                    Description: "This table has basic information about orders, as well as some derived facts based on payments",
                },
                "stg_customers": ModelDetails{
                    URI:         filepath.Join(testdataRoot, "models/staging/stg_customers.sql"),
                    ProjectName: "jaffle_shop",
                    Description: "",
                },
                "stg_orders": ModelDetails{
                    URI:         filepath.Join(testdataRoot, "models/staging/stg_orders.sql"),
                    ProjectName: "jaffle_shop",
                    Description: "",
                },
                "stg_payments": ModelDetails{
                    URI:         filepath.Join(testdataRoot, "models/staging/stg_payments.sql"),
                    ProjectName: "jaffle_shop",
                    Description: "",
                },
            },
            MacroDetailMap: map[string]Macro{},
            VariableMap:    map[string]interface {}{},
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
