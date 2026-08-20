package fusion

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/j-clemons/dbt-language-server/lsp"
)

func makeLocalTempDir(t *testing.T) string {
	t.Helper()

	tempDir, err := os.MkdirTemp(".", ".compile-test-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(tempDir); err != nil {
			t.Errorf("remove temp dir: %v", err)
		}
	})

	absoluteDir, err := filepath.Abs(tempDir)
	if err != nil {
		t.Fatal(err)
	}
	return absoluteDir
}

func TestBuildFusionCompileCommand(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	fusionPath := filepath.Join(tempDir, "bin", "dbt")
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	artifactPath := filepath.Join(tempDir, "artifacts")
	selector := "marts.orders"

	tests := []struct {
		name              string
		configureProfiles func(t *testing.T) string
	}{
		{
			name: "without configured profiles",
			configureProfiles: func(t *testing.T) string {
				t.Setenv("DBT_PROFILES_DIR", "")
				return ""
			},
		},
		{
			name: "with absolute configured profiles",
			configureProfiles: func(t *testing.T) string {
				profilesDir := filepath.Join(tempDir, "absolute-profiles")
				t.Setenv("DBT_PROFILES_DIR", profilesDir)
				return profilesDir
			},
		},
		{
			name: "with relative configured profiles",
			configureProfiles: func(t *testing.T) string {
				processRoot := filepath.Join(tempDir, "process-root")
				if err := os.MkdirAll(processRoot, 0755); err != nil {
					t.Fatal(err)
				}
				t.Chdir(processRoot)
				t.Setenv("DBT_PROFILES_DIR", filepath.Join("editor", "profiles"))
				return filepath.Join(processRoot, "editor", "profiles")
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			expectedProfilesDir := test.configureProfiles(t)

			cmd, err := buildFusionCompileCommand(fusionPath, projectRoot, selector, artifactPath)
			if err != nil {
				t.Fatal(err)
			}

			expectedArgs := []string{
				fusionPath,
				"compile",
				"-q",
				"--static-analysis", "on",
				"--log-format", "json",
				"--no-write-json",
				"--project-dir", projectRoot,
			}
			if expectedProfilesDir != "" {
				expectedArgs = append(expectedArgs, "--profiles-dir", expectedProfilesDir)
			}
			expectedArgs = append(
				expectedArgs,
				"--target-path", filepath.Join(artifactPath, "target"),
				"--log-path", filepath.Join(artifactPath, "log"),
				"--select", selector,
			)

			if !reflect.DeepEqual(cmd.Args, expectedArgs) {
				t.Fatalf("expected args %#v, got %#v", expectedArgs, cmd.Args)
			}
			if cmd.Dir != projectRoot {
				t.Fatalf("expected command cwd %q, got %q", projectRoot, cmd.Dir)
			}
		})
	}
}

func TestParseFusionLogMsg(t *testing.T) {
	var tests = []struct {
		input     string
		code      string
		level     string
		act_uri   string
		act_msg   string
		act_range lsp.Range
	}{
		{
			`\u001b[31m\u001b[1merror:\u001b[0m dbt0101: mismatched input '\u001b[33msum\u001b[0m' expecting one of '\u001b[33m,\u001b[0m', '\u001b[33mEXCEPT\u001b[0m', '\u001b[33mFROM\u001b[0m', '\u001b[33mGROUP\u001b[0m', '\u001b[33mHAVING\u001b[0m', '\u001b[33mINTERSECT\u001b[0m' ...\n  --> models/marts/orders.sql:25:9 (target/compiled/models/marts/orders.sql:25:9)`,
			"0101",
			"error",
			"models/marts/orders.sql",
			"mismatched input 'sum' expecting one of ',', 'EXCEPT', 'FROM', 'GROUP', 'HAVING', 'INTERSECT' ...",
			lsp.Range{
				Start: lsp.Position{
					Line:      24,
					Character: 8,
				},
				End: lsp.Position{
					Line:      24,
					Character: 8,
				},
			},
		},
		{
			"error: dbt1502: Failed to render SQL syntax error: unexpected `}`, expected end of variable block\n(in models/marts/orders.sql:11)\n  --> models/marts/orders.sql:11:41",
			"1502",
			"error",
			"models/marts/orders.sql",
			"Failed to render SQL syntax error: unexpected `}`, expected end of variable block",
			lsp.Range{
				Start: lsp.Position{
					Line:      10,
					Character: 40,
				},
				End: lsp.Position{
					Line:      10,
					Character: 40,
				},
			},
		},
	}

	for _, test := range tests {
		act_uri, act_msg, act_range := parseFusionLogMsg(test.input, test.code, test.level)
		if act_uri != test.act_uri {
			t.Errorf("Expected %s, got %s", test.act_uri, act_uri)
		}
		if act_msg != test.act_msg {
			t.Errorf("Expected %s, got %s", test.act_msg, act_msg)
		}
		if act_range != test.act_range {
			t.Errorf("Expected %v, got %v", test.act_range, act_range)
		}
	}
}
