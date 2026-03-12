package util

import (
	"os"
	"testing"
)

func TestResolveEnvVars(t *testing.T) {
	os.Setenv("TEST_DBT_PROFILE", "my_profile")
	defer os.Unsetenv("TEST_DBT_PROFILE")

	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "double quoted env var",
			input:    `profile: "{{ env_var("TEST_DBT_PROFILE") }}"`,
			expected: `profile: "my_profile"`,
		},
		{
			name:     "single quoted env var",
			input:    `profile: "{{ env_var('TEST_DBT_PROFILE') }}"`,
			expected: `profile: "my_profile"`,
		},
		{
			name:     "env var with extra spaces",
			input:    `profile: "{{  env_var( "TEST_DBT_PROFILE" )  }}"`,
			expected: `profile: "my_profile"`,
		},
		{
			name:     "env var with default value uses env",
			input:    `profile: "{{ env_var("TEST_DBT_PROFILE", "fallback") }}"`,
			expected: `profile: "my_profile"`,
		},
		{
			name:     "missing env var with default",
			input:    `profile: "{{ env_var("NONEXISTENT_VAR_12345", "fallback") }}"`,
			expected: `profile: "fallback"`,
		},
		{
			name:     "missing env var with empty default",
			input:    `profile: "{{ env_var("NONEXISTENT_VAR_12345", "") }}"`,
			expected: `profile: ""`,
		},
		{
			name:     "missing env var without default uses fallback",
			input:    `profile: "{{ env_var("NONEXISTENT_VAR_12345") }}"`,
			expected: `profile: "DBT_ENV_DEFAULT_NONEXISTENT_VAR_12345"`,
		},
		{
			name:     "no env var expression",
			input:    `profile: "my_profile"`,
			expected: `profile: "my_profile"`,
		},
		{
			name:     "multiple env vars",
			input:    `a: "{{ env_var("TEST_DBT_PROFILE") }}" b: "{{ env_var("TEST_DBT_PROFILE") }}"`,
			expected: `a: "my_profile" b: "my_profile"`,
		},
		{
			name:     "multiple missing env vars use distinct fallbacks",
			input:    `a: "{{ env_var("MISSING_A") }}" b: "{{ env_var("MISSING_B") }}"`,
			expected: `a: "DBT_ENV_DEFAULT_MISSING_A" b: "DBT_ENV_DEFAULT_MISSING_B"`,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ResolveEnvVars(tt.input)
			if result != tt.expected {
				t.Errorf("expected %q but got %q", tt.expected, result)
			}
		})
	}
}
