package analysis

import (
	"path/filepath"
	"reflect"
	"testing"

	"github.com/j-clemons/dbt-language-server/testutils"
)

func TestCreateModelPathMap(t *testing.T) {
    testdataRoot, err := testutils.GetTestdataPath("jaffle_shop_duckdb")
    if err != nil {
        panic(err)
    }
    expectedState := expectedTestState()

    modelPathMap := createModelPathMap(
        expectedState.DbtContext.ProjectRoot,
        expectedState.DbtContext.ProjectYaml,
    )

    expected := map[string]string {
        "customers": filepath.Join(testdataRoot, "models/customers.sql"),
        "orders": filepath.Join(testdataRoot, "models/orders.sql"),
        "stg_customers": filepath.Join(testdataRoot, "models/staging/stg_customers.sql"),
        "stg_orders": filepath.Join(testdataRoot, "models/staging/stg_orders.sql"),
        "stg_payments": filepath.Join(testdataRoot, "models/staging/stg_payments.sql"),
    }

    if !reflect.DeepEqual(modelPathMap, expected) {
        t.Fatalf("expected %v, got %v", expectedState.DbtContext.ModelDetailMap, modelPathMap)
    }
}
