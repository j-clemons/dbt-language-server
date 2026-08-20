package fusion

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"

	"github.com/j-clemons/dbt-language-server/analysis"
	"github.com/j-clemons/dbt-language-server/lsp"
	diagnosticseverity "github.com/j-clemons/dbt-language-server/lsp/diagnosticSeverity"
	"github.com/j-clemons/dbt-language-server/util"
)

type FusionLog struct {
	Data Data
	Info Info
}

type Data struct {
	LogVersion int
	Version    string
}

type Info struct {
	Category     string
	Code         string
	Extra        map[string]any
	InvocationID string
	Level        string
	Msg          string
	Name         string
	Pid          int
	Thread       string
	Ts           string
}

func publishDiagnostics(writer io.Writer, uri string, diagnostics []lsp.Diagnostic) {
	notification := lsp.DiagnosticsNotification{
		Notification: lsp.Notification{
			RPC:    "2.0",
			Method: "textDocument/publishDiagnostics",
		},
		Params: lsp.PublishDiagnosticsParams{
			URI:         uri,
			Diagnostics: diagnostics,
		},
	}

	util.WriteResponse(writer, notification)
}

func FusionCompile(s *analysis.State, uri string, logger *log.Logger, writer io.Writer) {
	if !s.IsFusionEnabled() {
		return
	}
	selector := dbtModelSelectionFromUri(uri)

	projectName := s.DbtContext.ProjectYaml.ProjectName.Value
	fusionArtifactPath, err := getFusionArtifactPath(projectName)
	if err != nil {
		logger.Printf("Failed to get fusion artifact path: %v", err)
		return
	}

	cmd, err := buildFusionCompileCommand(
		s.FusionPath,
		s.DbtContext.ProjectRoot,
		selector,
		fusionArtifactPath,
	)
	if err != nil {
		logger.Printf("Failed to construct Fusion compile command: %v", err)
		return
	}
	logger.Printf("Running: %v\n", cmd.Args)

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		logger.Printf("Failed to open Fusion stdout: %v", err)
		return
	}

	stderr, err := cmd.StderrPipe()
	if err != nil {
		logger.Printf("Failed to open Fusion stderr: %v", err)
		return
	}

	if err := cmd.Start(); err != nil {
		logger.Printf("Failed to start Fusion compile: %v", err)
		return
	}

	diagnosticsChan := make(chan lsp.Diagnostic, 100)
	diagnostics := []lsp.Diagnostic{}

	var wg sync.WaitGroup

	go func() {
		for diagnostic := range diagnosticsChan {
			diagnostics = append(diagnostics, diagnostic)
			publishDiagnostics(writer, uri, diagnostics)
		}
	}()

	wg.Add(1)
	go func() {
		defer wg.Done()
		processStream(stdout, uri, logger, diagnosticsChan, "stdout")
	}()

	wg.Add(1)
	go func() {
		defer wg.Done()
		processStream(stderr, uri, logger, diagnosticsChan, "stderr")
	}()

	go func() {
		wg.Wait()
		close(diagnosticsChan)
	}()

	if err := cmd.Wait(); err != nil {
		logger.Printf("Command failed: %v", err)
	}

	publishDiagnostics(writer, uri, diagnostics)
}

func buildFusionCompileCommand(fusionPath string, projectRoot string, selector string, artifactPath string) (*exec.Cmd, error) {
	if projectRoot == "" {
		return nil, fmt.Errorf("dbt project root is empty")
	}

	args := []string{
		"compile",
		"-q",
		"--static-analysis", "on",
		"--log-format", "json",
		"--no-write-json",
		"--project-dir", projectRoot,
	}

	profilesDir, configured, err := util.ResolveConfiguredProfilesDir()
	if err != nil {
		return nil, fmt.Errorf("resolve DBT_PROFILES_DIR: %w", err)
	}
	if configured {
		args = append(args, "--profiles-dir", profilesDir)
	}

	args = append(
		args,
		"--target-path", filepath.Join(artifactPath, "target"),
		"--log-path", filepath.Join(artifactPath, "log"),
		"--select", selector,
	)

	cmd := exec.Command(fusionPath, args...)
	cmd.Dir = projectRoot
	return cmd, nil
}

func processStream(stream io.Reader, uri string, logger *log.Logger, diagnosticsChan chan lsp.Diagnostic, streamName string) {
	scanner := bufio.NewScanner(stream)
	for scanner.Scan() {
		line := scanner.Bytes()
		logger.Printf("[%s] %s", streamName, string(line))

		var entry FusionLog
		if err := json.Unmarshal(line, &entry); err != nil {
			logger.Printf("[%s] failed to parse JSON: %v", streamName, err)
			continue
		}

		diagnosticUri, diagnostic := parseCompileLog(entry)
		if strings.Contains(uri, diagnosticUri) {
			diagnosticsChan <- diagnostic
		}
	}

	if err := scanner.Err(); err != nil {
		logger.Printf("[%s] scanner error: %v", streamName, err)
	}
}

func parseCompileLog(log FusionLog) (string, lsp.Diagnostic) {
	var severity int
	switch log.Info.Level {
	case "info":
		severity = diagnosticseverity.Info
	case "warning":
		severity = diagnosticseverity.Warning
	case "error":
		severity = diagnosticseverity.Error
	default:
		severity = diagnosticseverity.Hint
	}

	uri, msg, diagRange := parseFusionLogMsg(log.Info.Msg, log.Info.Code, log.Info.Level)

	diagnostic := lsp.Diagnostic{
		Range:    diagRange,
		Message:  msg,
		Severity: severity,
		Code:     log.Info.Code,
		Source:   fmt.Sprintf("dbt Fusion %s", log.Data.Version),
	}

	return uri, diagnostic
}

func parseFusionLogMsg(msg string, code string, level string) (string, string, lsp.Range) {
	colorCodeRegex := regexp.MustCompile(`(\\u001b\[[0-9;]*m|\x1b\[[0-9;]*m)`)
	cleanMsg := colorCodeRegex.ReplaceAllString(msg, "")

	parts := strings.Split(cleanMsg, " --> ")
	if len(parts) != 2 {
		return "NO URI FOUND", cleanMsg, lsp.Range{}
	}

	message := strings.ReplaceAll(strings.TrimSpace(parts[0]), `\n`, "")
	levelRegex := regexp.MustCompile(fmt.Sprintf(`^%s: `, level))
	codeRegex := regexp.MustCompile(fmt.Sprintf(`^%s: |^dbt%s: `, code, code))
	modelPathRegex := regexp.MustCompile(`(\(in .*:\d*\))`)

	message = levelRegex.ReplaceAllString(message, "")
	message = codeRegex.ReplaceAllString(message, "")
	message = modelPathRegex.ReplaceAllString(message, "")
	message = strings.TrimSpace(message)

	locationPart := strings.TrimSpace(parts[1])
	locationParts := strings.Split(locationPart, " ")
	filePath := locationParts[0]

	pathParts := strings.Split(filePath, ":")
	if len(pathParts) < 3 {
		return "NO URI FOUND", message, lsp.Range{}
	}

	uri := pathParts[0]

	line, err := strconv.Atoi(pathParts[1])
	if err != nil {
		line = 0
	} else {
		line--
	}

	character, err := strconv.Atoi(pathParts[2])
	if err != nil {
		character = 0
	} else {
		character--
	}

	position := lsp.Position{
		Line:      line,
		Character: character,
	}

	diagnosticRange := lsp.Range{
		Start: position,
		End:   position,
	}

	return uri, message, diagnosticRange
}

func dbtModelSelectionFromUri(uri string) string {
	lastModelIdx := strings.LastIndex(uri, "models")
	if lastModelIdx == -1 {
		return "*"
	}

	modelSelector := strings.ReplaceAll(
		strings.TrimSuffix(
			strings.TrimPrefix(
				uri[lastModelIdx+len("models"):],
				"/",
			),
			".sql",
		),
		"/",
		".",
	)

	return modelSelector
}

func getFusionArtifactPath(projectName string) (string, error) {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}

	fusionArtifactPath := filepath.Join(homeDir, ".dbt", "dbt-language-server", "fusion-artifacts", projectName)

	if err := os.MkdirAll(fusionArtifactPath, 0755); err != nil {
		return "", err
	}

	logDir := filepath.Join(fusionArtifactPath, "log")
	if err := os.MkdirAll(logDir, 0755); err != nil {
		return "", err
	}

	targetDir := filepath.Join(fusionArtifactPath, "target")
	if err := os.MkdirAll(targetDir, 0755); err != nil {
		return "", err
	}

	return fusionArtifactPath, nil
}
