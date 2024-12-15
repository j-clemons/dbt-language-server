package analysis

import (
    "reflect"
    "testing"
)

func TestGetProjectVariables(t *testing.T) {
    expectedState := expectedTestState()

    projectVariables := getProjectVariables(expectedState.DbtContext.ProjectYaml)

    if !reflect.DeepEqual(projectVariables, expectedState.DbtContext.VariableMap) {
        t.Fatalf("expected %v, got %v", expectedState.DbtContext.VariableMap, projectVariables)
    }
}
