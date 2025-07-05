package analysis

import (
	"testing"
)

func TestGetDocsFileContents(t *testing.T) {
	docsFileStr := `
{% docs table_events %}

This table contains clickstream events from the marketing website.

{% enddocs %}
`
	testCases := []struct {
		name     string
		fileStr  string
		expected []Docs
	}{
		{
			name:    "Example Doc File",
			fileStr: docsFileStr,
			expected: []Docs{
				Docs{
					Name:    "table_events",
					Content: "This table contains clickstream events from the marketing website.",
				},
			},
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := getDocsFileContents(tc.fileStr)
			for i, e := range tc.expected {
				if e != result[i] {
					t.Errorf("input: %v; got: %v; want: %v",
						tc.fileStr, result[i], e)
				}
			}
		})
	}
}
