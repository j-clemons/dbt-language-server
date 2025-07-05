package analysis

import (
	"reflect"
	"sort"
	"testing"

	"github.com/j-clemons/dbt-language-server/lsp"
	"github.com/j-clemons/dbt-language-server/lsp/completionKind"
)

func TestReverseRefPrefix(t *testing.T) {
	testCases := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "Full Jinja",
			input:    "{{ ('",
			expected: "') }}",
		},
		{
			name:     "Multiple Spaces",
			input:    "{{   ('",
			expected: "')   }}",
		},
		{
			name:     "No Spaces",
			input:    "{{('",
			expected: "')}}",
		},
		{
			name:     "Generic Reversal",
			input:    "reversal",
			expected: "lasrever",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := reverseRefPrefix(tc.input)
			if result != tc.expected {
				t.Errorf("input: %s; got: %s; want: %s",
					tc.input, result, tc.expected)
			}
		})
	}
}

func TestGetReferenceSuffix(t *testing.T) {
	testCases := []struct {
		name     string
		ref      string
		trailing string
		expected string
	}{
		{
			name:     "Full Jinja",
			ref:      "{{ ref('",
			trailing: "",
			expected: "') }}",
		},
		{
			name:     "Multiple Spaces",
			ref:      "{{   ref('",
			trailing: "",
			expected: "')   }}",
		},
		{
			name:     "Trailing Jinja Symbols",
			ref:      "{{ ref('",
			trailing: "') }}",
			expected: "",
		},
		{
			name:     "Trailing Characters",
			ref:      "{{ ref('",
			trailing: "')",
			expected: "",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := getSuffix(tc.ref, tc.trailing, "ref")
			if result != tc.expected {
				t.Errorf("input: %s, %s; got: %s; want: %s",
					tc.ref, tc.trailing, result, tc.expected)
			}
		})
	}
}

func TestGetVariableSuffix(t *testing.T) {
	testCases := []struct {
		name     string
		vars     string
		trailing string
		expected string
	}{
		{
			name:     "Full Jinja",
			vars:     "{{ var('",
			trailing: "",
			expected: "') }}",
		},
		{
			name:     "Multiple Spaces",
			vars:     "{{   var('",
			trailing: "",
			expected: "')   }}",
		},
		{
			name:     "Trailing Jinja Symbols",
			vars:     "{{ var('",
			trailing: "') }}",
			expected: "",
		},
		{
			name:     "Trailing Characters",
			vars:     "{{ var('",
			trailing: "')",
			expected: "",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := getSuffix(tc.vars, tc.trailing, "var")
			if result != tc.expected {
				t.Errorf("input: %s, %s; got: %s; want: %s",
					tc.vars, tc.trailing, result, tc.expected)
			}
		})
	}
}

func TestGetMacroCompletionItems(t *testing.T) {
	testState := expectedTestState()

	actualCompletionItems := getMacroCompletionItems(
		testState.DbtContext.MacroDetailMap,
		testState.DbtContext.ProjectYaml,
	)

	expectedCompletionItems := []lsp.CompletionItem{
		{
			Label:         "add_values",
			Detail:        "Project: jaffle_package",
			Documentation: "add_values(arg1, arg2)",
			Kind:          completionKind.Snippet,
			InsertText:    "jaffle_package.add_values",
			SortText:      "add_values",
		},
		{
			Label:         "full_name",
			Detail:        "Project: jaffle_shop",
			Documentation: "full_name(first_name, last_name)",
			Kind:          completionKind.Snippet,
			InsertText:    "full_name",
			SortText:      "full_name",
		},
		{
			Label:         "times_five",
			Detail:        "Project: jaffle_shop",
			Documentation: "times_five(int_value)",
			Kind:          completionKind.Snippet,
			InsertText:    "times_five",
			SortText:      "times_five",
		},
	}

	sort.Slice(actualCompletionItems, func(i, j int) bool {
		return actualCompletionItems[i].Label < actualCompletionItems[j].Label
	})

	sort.Slice(expectedCompletionItems, func(i, j int) bool {
		return expectedCompletionItems[i].Label < expectedCompletionItems[j].Label
	})

	if !reflect.DeepEqual(actualCompletionItems, expectedCompletionItems) {
		t.Fatalf("expected %v,\n\ngot %v", expectedCompletionItems, actualCompletionItems)
	}
}
