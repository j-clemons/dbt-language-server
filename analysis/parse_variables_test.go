package analysis

import (
    "reflect"
    "testing"
)

func TestGetProjectVariables(t *testing.T) {
    expectedState := expectedTestState()

    projectVariables := getProjectVariables(
        expectedState.DbtContext.ProjectYaml,
        expectedState.DbtContext.ProjectRoot,
    )

    if !reflect.DeepEqual(projectVariables, expectedState.DbtContext.VariableDetailMap) {
        t.Fatalf("expected %v, got %v", expectedState.DbtContext.VariableDetailMap, projectVariables)
    }
}
