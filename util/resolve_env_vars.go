package util

import (
	"os"
	"regexp"
)

var envVarRegex = regexp.MustCompile(`\{\{\s*env_var\(\s*('|")([^'"]+)('|")\s*(?:,\s*('|")([^'"]*?)('|"))?\s*\)\s*\}\}`)

func ResolveEnvVars(input string) string {
	return envVarRegex.ReplaceAllStringFunc(input, func(match string) string {
		submatches := envVarRegex.FindStringSubmatch(match)
		varName := submatches[2]
		defaultValue := submatches[5]

		value, exists := os.LookupEnv(varName)
		if exists {
			return value
		}

		if submatches[4] != "" {
			return defaultValue
		}

		return match
	})
}
