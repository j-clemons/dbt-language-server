package analysis

import (
    "testing"
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
            result := getReferenceSuffix(tc.ref, tc.trailing)
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
            result := getVariableSuffix(tc.vars, tc.trailing)
            if result != tc.expected {
                t.Errorf("input: %s, %s; got: %s; want: %s",
                    tc.vars, tc.trailing, result, tc.expected)
            }
        })
    }
}
