package fusion

import (
	"bytes"
	"log"
	"net/url"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"

	"github.com/j-clemons/dbt-language-server/analysis"
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

func writeProfilesFile(t *testing.T, profilesDir string) string {
	t.Helper()

	if err := os.MkdirAll(profilesDir, 0755); err != nil {
		t.Fatal(err)
	}
	profilesPath := filepath.Join(profilesDir, "profiles.yml")
	if err := os.WriteFile(profilesPath, []byte("{}\n"), 0644); err != nil {
		t.Fatal(err)
	}
	return profilesPath
}

func clearHomeEnvironment(t *testing.T) {
	t.Helper()

	t.Setenv("HOME", "")
	t.Setenv("USERPROFILE", "")
	t.Setenv("HOMEDRIVE", "")
	t.Setenv("HOMEPATH", "")
}

func setHomeEnvironment(t *testing.T, homeDir string) {
	t.Helper()

	t.Setenv("HOME", homeDir)
	t.Setenv("USERPROFILE", homeDir)
	volume := filepath.VolumeName(homeDir)
	t.Setenv("HOMEDRIVE", volume)
	t.Setenv("HOMEPATH", strings.TrimPrefix(homeDir, volume))
}

func fileURIForPath(path string) string {
	uriPath := filepath.ToSlash(path)
	if runtime.GOOS == "windows" && !strings.HasPrefix(uriPath, "/") {
		uriPath = "/" + uriPath
	}
	return (&url.URL{Scheme: "file", Path: uriPath}).String()
}

func assertDirectoryExists(t *testing.T, path string) {
	t.Helper()

	fileInfo, err := os.Stat(path)
	if err != nil {
		t.Fatalf("expected directory %q: %v", path, err)
	}
	if !fileInfo.IsDir() {
		t.Fatalf("expected %q to be a directory", path)
	}
}

func TestGetFusionArtifactPathUsesHome(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	homeDir := filepath.Join(tempDir, "home")
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	setHomeEnvironment(t, homeDir)

	artifactPath, err := getFusionArtifactPath(projectRoot, "nested_project")
	if err != nil {
		t.Fatal(err)
	}

	expectedPath := filepath.Join(
		homeDir,
		".dbt",
		"dbt-language-server",
		"fusion-artifacts",
		"nested_project",
	)
	if artifactPath != expectedPath {
		t.Fatalf("expected artifact path %q, got %q", expectedPath, artifactPath)
	}
	assertDirectoryExists(t, artifactPath)
	assertDirectoryExists(t, filepath.Join(artifactPath, "log"))
	assertDirectoryExists(t, filepath.Join(artifactPath, "target"))
}

func TestGetFusionArtifactPathFallsBackToProjectWithoutHome(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	if err := os.MkdirAll(projectRoot, 0755); err != nil {
		t.Fatal(err)
	}
	clearHomeEnvironment(t)

	artifactPath, err := getFusionArtifactPath(projectRoot, "nested_project")
	if err != nil {
		t.Fatal(err)
	}

	expectedPath := filepath.Join(
		projectRoot,
		"target",
		".dbt-language-server",
		"fusion-artifacts",
		"nested_project",
	)
	if artifactPath != expectedPath {
		t.Fatalf("expected artifact path %q, got %q", expectedPath, artifactPath)
	}
	assertDirectoryExists(t, artifactPath)
	assertDirectoryExists(t, filepath.Join(artifactPath, "log"))
	assertDirectoryExists(t, filepath.Join(artifactPath, "target"))
}

func TestGetFusionArtifactPathRejectsUnsafeProjectName(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	setHomeEnvironment(t, filepath.Join(tempDir, "home"))

	for _, projectName := range []string{"", ".", "..", "../escape", `..\escape`, "nested/project", `nested\project`, "C:escape"} {
		t.Run(projectName, func(t *testing.T) {
			_, err := getFusionArtifactPath(filepath.Join(tempDir, "repository", "dbt"), projectName)
			if err == nil {
				t.Fatalf("expected unsafe project name %q to be rejected", projectName)
			}
		})
	}
}

func TestGetFusionArtifactPathReturnsDirectoryCreationError(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	homeDir := filepath.Join(tempDir, "home")
	if err := os.MkdirAll(homeDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(homeDir, ".dbt"), []byte("not a directory\n"), 0644); err != nil {
		t.Fatal(err)
	}
	setHomeEnvironment(t, homeDir)

	_, err := getFusionArtifactPath(filepath.Join(tempDir, "repository", "dbt"), "nested_project")
	if err == nil {
		t.Fatal("expected directory creation error")
	}
	if !strings.Contains(err.Error(), "create Fusion artifact directory") {
		t.Fatalf("expected descriptive directory creation error, got %v", err)
	}
}

func TestBuildFusionCompileCommand(t *testing.T) {
	tests := []struct {
		name              string
		configureProfiles func(t *testing.T, tempDir string, projectRoot string) string
	}{
		{
			name: "configured absolute profiles",
			configureProfiles: func(t *testing.T, tempDir string, projectRoot string) string {
				profilesDir := filepath.Join(tempDir, "absolute-profiles")
				writeProfilesFile(t, profilesDir)
				t.Setenv("DBT_PROFILES_DIR", profilesDir)
				return profilesDir
			},
		},
		{
			name: "configured relative profiles",
			configureProfiles: func(t *testing.T, tempDir string, projectRoot string) string {
				processRoot := filepath.Join(tempDir, "process-root")
				if err := os.MkdirAll(processRoot, 0755); err != nil {
					t.Fatal(err)
				}
				t.Chdir(processRoot)
				profilesDir := filepath.Join(processRoot, "editor", "profiles")
				writeProfilesFile(t, profilesDir)
				t.Setenv("DBT_PROFILES_DIR", filepath.Join("editor", "profiles"))
				return profilesDir
			},
		},
		{
			name: "project profiles",
			configureProfiles: func(t *testing.T, tempDir string, projectRoot string) string {
				t.Setenv("DBT_PROFILES_DIR", "")
				clearHomeEnvironment(t)
				writeProfilesFile(t, projectRoot)
				return projectRoot
			},
		},
		{
			name: "project dot dbt profiles",
			configureProfiles: func(t *testing.T, tempDir string, projectRoot string) string {
				t.Setenv("DBT_PROFILES_DIR", "")
				clearHomeEnvironment(t)
				profilesDir := filepath.Join(projectRoot, ".dbt")
				writeProfilesFile(t, profilesDir)
				return profilesDir
			},
		},
		{
			name: "home profiles",
			configureProfiles: func(t *testing.T, tempDir string, projectRoot string) string {
				t.Setenv("DBT_PROFILES_DIR", "")
				homeDir := filepath.Join(tempDir, "home")
				setHomeEnvironment(t, homeDir)
				profilesDir := filepath.Join(homeDir, ".dbt")
				writeProfilesFile(t, profilesDir)
				return profilesDir
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			tempDir := makeLocalTempDir(t)
			fusionPath := filepath.Join(tempDir, "bin", "dbt")
			projectRoot := filepath.Join(tempDir, "repository", "dbt")
			artifactPath := filepath.Join(tempDir, "artifacts")
			selector := "path:models/marts/orders.sql"
			if err := os.MkdirAll(projectRoot, 0755); err != nil {
				t.Fatal(err)
			}

			expectedProfilesDir := test.configureProfiles(t, tempDir, projectRoot)

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
				"--profiles-dir", expectedProfilesDir,
				"--target-path", filepath.Join(artifactPath, "target"),
				"--log-path", filepath.Join(artifactPath, "log"),
				"--select", selector,
			}

			if !reflect.DeepEqual(cmd.Args, expectedArgs) {
				t.Fatalf("expected args %#v, got %#v", expectedArgs, cmd.Args)
			}
			if cmd.Dir != projectRoot {
				t.Fatalf("expected command cwd %q, got %q", projectRoot, cmd.Dir)
			}
		})
	}
}

func TestBuildFusionCompileCommandRequiresProfiles(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	if err := os.MkdirAll(projectRoot, 0755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("DBT_PROFILES_DIR", "")
	clearHomeEnvironment(t)

	_, err := buildFusionCompileCommand(
		filepath.Join(tempDir, "bin", "dbt"),
		projectRoot,
		"path:models/orders.sql",
		filepath.Join(tempDir, "artifacts"),
	)
	if err == nil {
		t.Fatal("expected missing profiles error")
	}
	if !strings.Contains(err.Error(), "profiles.yml") {
		t.Fatalf("expected descriptive profiles error, got %v", err)
	}
}

func TestDbtPathSelectionFromURI(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	modelPaths := []string{"models", "functions", "sources", "ml_models"}
	if err := os.MkdirAll(projectRoot, 0755); err != nil {
		t.Fatal(err)
	}

	tests := []struct {
		name         string
		path         string
		wantSelector string
	}{
		{
			name:         "model",
			path:         filepath.Join(projectRoot, "models", "marts", "orders.sql"),
			wantSelector: "path:models/marts/orders.sql",
		},
		{
			name:         "function resource",
			path:         filepath.Join(projectRoot, "functions", "calculate_total.sql"),
			wantSelector: "path:functions/calculate_total.sql",
		},
		{
			name:         "source resource",
			path:         filepath.Join(projectRoot, "sources", "external_orders.sql"),
			wantSelector: "path:sources/external_orders.sql",
		},
		{
			name:         "custom model resource directory",
			path:         filepath.Join(projectRoot, "ml_models", "recommendations.sql"),
			wantSelector: "path:ml_models/recommendations.sql",
		},
		{
			name:         "encoded path",
			path:         filepath.Join(projectRoot, "models", "customer orders.sql"),
			wantSelector: "path:models/customer orders.sql",
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if err := os.MkdirAll(filepath.Dir(test.path), 0755); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(test.path, []byte("select 1\n"), 0644); err != nil {
				t.Fatal(err)
			}

			selector, err := dbtPathSelectionFromURI(fileURIForPath(test.path), projectRoot, modelPaths)
			if err != nil {
				t.Fatal(err)
			}
			if selector != test.wantSelector {
				t.Fatalf("expected selector %q, got %q", test.wantSelector, selector)
			}
		})
	}
}

func TestDbtPathSelectionFromURIRejectsUnsupportedDocuments(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	modelPaths := []string{"models", "functions", "sources", "ml_models"}
	sqlDirectory := filepath.Join(projectRoot, "models", "not-a-file.sql")
	if err := os.MkdirAll(sqlDirectory, 0755); err != nil {
		t.Fatal(err)
	}
	unrelatedSQL := filepath.Join(projectRoot, "notes", "query.sql")
	targetSQL := filepath.Join(projectRoot, "target", "compiled", "orders.sql")
	modelsBoundarySQL := filepath.Join(projectRoot, "models_archive", "orders.sql")
	for _, path := range []string{unrelatedSQL, targetSQL, modelsBoundarySQL} {
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte("select 1\n"), 0644); err != nil {
			t.Fatal(err)
		}
	}

	tests := []struct {
		name string
		uri  string
	}{
		{
			name: "yaml",
			uri:  fileURIForPath(filepath.Join(projectRoot, "models", "schema.yml")),
		},
		{
			name: "python",
			uri:  fileURIForPath(filepath.Join(projectRoot, "models", "forecast.py")),
		},
		{
			name: "sql outside project",
			uri:  fileURIForPath(filepath.Join(tempDir, "outside", "orders.sql")),
		},
		{
			name: "sql directory",
			uri:  fileURIForPath(sqlDirectory),
		},
		{
			name: "unrelated in-project sql",
			uri:  fileURIForPath(unrelatedSQL),
		},
		{
			name: "target compiled sql",
			uri:  fileURIForPath(targetSQL),
		},
		{
			name: "model path segment boundary",
			uri:  fileURIForPath(modelsBoundarySQL),
		},
		{
			name: "untitled URI",
			uri:  "untitled:models/orders.sql",
		},
		{
			name: "opaque file URI",
			uri:  "file:models/orders.sql",
		},
		{
			name: "plain path",
			uri:  filepath.Join(projectRoot, "models", "orders.sql"),
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			selector, err := dbtPathSelectionFromURI(test.uri, projectRoot, modelPaths)
			if err == nil {
				t.Fatalf("expected URI %q to be rejected, got selector %q", test.uri, selector)
			}
		})
	}
}

func TestDbtPathSelectionFromURIRejectsSymlinkOutsideProject(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	modelPaths := []string{"models", "functions", "sources", "ml_models"}
	modelsDir := filepath.Join(projectRoot, "models")
	if err := os.MkdirAll(modelsDir, 0755); err != nil {
		t.Fatal(err)
	}
	outsidePath := filepath.Join(tempDir, "outside.sql")
	if err := os.WriteFile(outsidePath, []byte("select 1\n"), 0644); err != nil {
		t.Fatal(err)
	}
	symlinkPath := filepath.Join(modelsDir, "outside.sql")
	if err := os.Symlink(outsidePath, symlinkPath); err != nil {
		t.Skipf("symlink not supported: %v", err)
	}

	selector, err := dbtPathSelectionFromURI(fileURIForPath(symlinkPath), projectRoot, modelPaths)
	if err == nil {
		t.Fatalf("expected outside symlink to be rejected, got selector %q", selector)
	}
}

func TestDiagnosticPathMatchesPercentEncodedDocumentURI(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	projectRoot := filepath.Join(tempDir, "repository", "dbt")
	modelPath := filepath.Join(projectRoot, "models", "customer orders.sql")
	if err := os.MkdirAll(filepath.Dir(modelPath), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(modelPath, []byte("select 1\n"), 0644); err != nil {
		t.Fatal(err)
	}

	selector, err := dbtPathSelectionFromURI(
		fileURIForPath(modelPath),
		projectRoot,
		[]string{"models"},
	)
	if err != nil {
		t.Fatal(err)
	}
	documentRelativePath := strings.TrimPrefix(selector, "path:")

	if !diagnosticPathMatchesDocument(documentRelativePath, "models/customer orders.sql") {
		t.Fatalf("expected decoded document path %q to match diagnostic path", documentRelativePath)
	}
	if diagnosticPathMatchesDocument(documentRelativePath, "models/customer.sql") {
		t.Fatal("expected different diagnostic path not to match")
	}
}

func TestCollectDiagnosticsCompletes(t *testing.T) {
	diagnosticsChan := make(chan lsp.Diagnostic, 2)
	first := lsp.Diagnostic{Message: "first"}
	second := lsp.Diagnostic{Message: "second"}
	diagnosticsChan <- first
	diagnosticsChan <- second
	close(diagnosticsChan)

	var writer bytes.Buffer
	diagnostics := collectDiagnostics(diagnosticsChan, &writer, "file:///models/orders.sql")

	if !reflect.DeepEqual(diagnostics, []lsp.Diagnostic{first, second}) {
		t.Fatalf("expected collected diagnostics, got %#v", diagnostics)
	}
	if writer.Len() == 0 {
		t.Fatal("expected diagnostic notifications to be written before collector returned")
	}
}

func TestFusionCompileSkipsUnsupportedURI(t *testing.T) {
	tempDir := makeLocalTempDir(t)
	state := analysis.NewState()
	state.SetFusionEnabled(true)
	state.FusionPath = filepath.Join(tempDir, "missing-dbt")
	state.DbtContext.ProjectRoot = filepath.Join(tempDir, "repository", "dbt")

	var logOutput bytes.Buffer
	var writer bytes.Buffer
	FusionCompile(
		&state,
		fileURIForPath(filepath.Join(state.DbtContext.ProjectRoot, "models", "schema.yml")),
		log.New(&logOutput, "", 0),
		&writer,
	)

	if writer.Len() != 0 {
		t.Fatalf("expected no JSON-RPC writes, got %q", writer.String())
	}
	if !strings.Contains(logOutput.String(), "Skipping Fusion compile") {
		t.Fatalf("expected skip log, got %q", logOutput.String())
	}
	if strings.Contains(logOutput.String(), "Running:") {
		t.Fatalf("expected no Fusion subprocess, got %q", logOutput.String())
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
		{
			"error: dbt1502: Invalid SQL\n  --> models/customer orders.sql:7:3",
			"1502",
			"error",
			"models/customer orders.sql",
			"Invalid SQL",
			lsp.Range{
				Start: lsp.Position{
					Line:      6,
					Character: 2,
				},
				End: lsp.Position{
					Line:      6,
					Character: 2,
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
