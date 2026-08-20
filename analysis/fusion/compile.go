package fusion

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
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

	selector, err := dbtPathSelectionFromURI(
		uri,
		s.DbtContext.ProjectRoot,
		s.DbtContext.ProjectYaml.ModelPaths.Value,
	)
	if err != nil {
		logger.Printf("Skipping Fusion compile for %q: %v", uri, err)
		return
	}
	documentRelativePath := strings.TrimPrefix(selector, "path:")

	projectName := s.DbtContext.ProjectYaml.ProjectName.Value
	fusionArtifactPath, err := getFusionArtifactPath(s.DbtContext.ProjectRoot, projectName)
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
	collectorDone := make(chan []lsp.Diagnostic, 1)
	go func() {
		collectorDone <- collectDiagnostics(diagnosticsChan, writer, uri)
	}()

	var readers sync.WaitGroup
	readers.Add(1)
	go func() {
		defer readers.Done()
		processStream(stdout, documentRelativePath, logger, diagnosticsChan, "stdout")
	}()

	readers.Add(1)
	go func() {
		defer readers.Done()
		processStream(stderr, documentRelativePath, logger, diagnosticsChan, "stderr")
	}()

	readers.Wait()
	close(diagnosticsChan)
	diagnostics := <-collectorDone

	if err := cmd.Wait(); err != nil {
		logger.Printf("Command failed: %v", err)
	}

	publishDiagnostics(writer, uri, diagnostics)
}

func collectDiagnostics(diagnosticsChan <-chan lsp.Diagnostic, writer io.Writer, uri string) []lsp.Diagnostic {
	diagnostics := []lsp.Diagnostic{}
	for diagnostic := range diagnosticsChan {
		diagnostics = append(diagnostics, diagnostic)
		publishDiagnostics(writer, uri, diagnostics)
	}
	return diagnostics
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

	profilesPath, err := util.ResolveProfilesPath(projectRoot)
	if err != nil {
		return nil, fmt.Errorf("resolve profiles.yml for Fusion compile: %w", err)
	}
	args = append(args, "--profiles-dir", filepath.Dir(profilesPath))

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

func processStream(stream io.Reader, documentRelativePath string, logger *log.Logger, diagnosticsChan chan lsp.Diagnostic, streamName string) {
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
		if diagnosticPathMatchesDocument(documentRelativePath, diagnosticUri) {
			diagnosticsChan <- diagnostic
		}
	}

	if err := scanner.Err(); err != nil {
		logger.Printf("[%s] scanner error: %v", streamName, err)
	}
}

func diagnosticPathMatchesDocument(documentRelativePath string, diagnosticPath string) bool {
	normalizedDocumentPath := filepath.Clean(filepath.FromSlash(documentRelativePath))
	normalizedDiagnosticPath := filepath.Clean(filepath.FromSlash(diagnosticPath))
	if filepath.IsAbs(normalizedDocumentPath) || filepath.IsAbs(normalizedDiagnosticPath) {
		return false
	}
	return normalizedDocumentPath == normalizedDiagnosticPath
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
	locationRegex := regexp.MustCompile(`^(.+?):(\d+):(\d+)(?:\s|$)`)
	locationParts := locationRegex.FindStringSubmatch(locationPart)
	if len(locationParts) != 4 {
		return "NO URI FOUND", message, lsp.Range{}
	}

	uri := locationParts[1]

	line, err := strconv.Atoi(locationParts[2])
	if err != nil {
		line = 0
	} else {
		line--
	}

	character, err := strconv.Atoi(locationParts[3])
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

func dbtPathSelectionFromURI(uri string, projectRoot string, modelPaths []string) (string, error) {
	if projectRoot == "" {
		return "", fmt.Errorf("dbt project root is empty")
	}

	filePath, err := filePathFromURI(uri)
	if err != nil {
		return "", err
	}
	if filepath.Ext(filePath) != ".sql" {
		return "", fmt.Errorf("unsupported file extension %q", filepath.Ext(filePath))
	}

	absoluteProjectRoot, err := filepath.Abs(projectRoot)
	if err != nil {
		return "", fmt.Errorf("resolve dbt project root %q: %w", projectRoot, err)
	}

	relativePath, err := filepath.Rel(absoluteProjectRoot, filePath)
	if err != nil {
		return "", fmt.Errorf("resolve %q relative to dbt project %q: %w", filePath, absoluteProjectRoot, err)
	}
	if relativePath == "." || relativePath == ".." || strings.HasPrefix(relativePath, ".."+string(filepath.Separator)) || filepath.IsAbs(relativePath) {
		return "", fmt.Errorf("sql file %q is outside dbt project %q", filePath, absoluteProjectRoot)
	}

	fileInfo, err := os.Stat(filePath)
	if err != nil {
		return "", fmt.Errorf("inspect sql file %q: %w", filePath, err)
	}
	if !fileInfo.Mode().IsRegular() {
		return "", fmt.Errorf("sql path %q is not a regular file", filePath)
	}

	resolvedProjectRoot, err := filepath.EvalSymlinks(absoluteProjectRoot)
	if err != nil {
		return "", fmt.Errorf("resolve dbt project root symlinks %q: %w", absoluteProjectRoot, err)
	}
	resolvedFilePath, err := filepath.EvalSymlinks(filePath)
	if err != nil {
		return "", fmt.Errorf("resolve sql file symlinks %q: %w", filePath, err)
	}
	resolvedRelativePath, err := filepath.Rel(resolvedProjectRoot, resolvedFilePath)
	if err != nil {
		return "", fmt.Errorf("resolve %q relative to dbt project %q: %w", resolvedFilePath, resolvedProjectRoot, err)
	}
	if resolvedRelativePath == "." || resolvedRelativePath == ".." || strings.HasPrefix(resolvedRelativePath, ".."+string(filepath.Separator)) || filepath.IsAbs(resolvedRelativePath) {
		return "", fmt.Errorf("sql file %q resolves outside dbt project %q", filePath, absoluteProjectRoot)
	}

	withinModelPath, err := fileWithinConfiguredModelPaths(
		filePath,
		resolvedFilePath,
		absoluteProjectRoot,
		modelPaths,
	)
	if err != nil {
		return "", err
	}
	if !withinModelPath {
		return "", fmt.Errorf("sql file %q is outside configured model paths %v", filePath, modelPaths)
	}

	return "path:" + filepath.ToSlash(relativePath), nil
}

func fileWithinConfiguredModelPaths(filePath string, resolvedFilePath string, projectRoot string, modelPaths []string) (bool, error) {
	for _, modelPath := range modelPaths {
		configuredPath := filepath.Clean(filepath.FromSlash(modelPath))
		if filepath.IsAbs(configuredPath) {
			continue
		}

		modelRoot := filepath.Join(projectRoot, configuredPath)
		withinModelRoot, err := pathWithinRoot(modelRoot, filePath)
		if err != nil {
			return false, fmt.Errorf("check configured model path %q: %w", modelPath, err)
		}
		if !withinModelRoot {
			continue
		}

		resolvedModelRoot, err := filepath.EvalSymlinks(modelRoot)
		if err != nil {
			return false, fmt.Errorf("resolve configured model path symlinks %q: %w", modelRoot, err)
		}
		withinResolvedModelRoot, err := pathWithinRoot(resolvedModelRoot, resolvedFilePath)
		if err != nil {
			return false, fmt.Errorf("check resolved configured model path %q: %w", resolvedModelRoot, err)
		}
		if withinResolvedModelRoot {
			return true, nil
		}
	}

	return false, nil
}

func pathWithinRoot(root string, path string) (bool, error) {
	relativePath, err := filepath.Rel(root, path)
	if err != nil {
		return false, err
	}
	return relativePath != ".." &&
		!strings.HasPrefix(relativePath, ".."+string(filepath.Separator)) &&
		!filepath.IsAbs(relativePath), nil
}

func filePathFromURI(uri string) (string, error) {
	parsedURI, err := url.Parse(uri)
	if err != nil {
		return "", fmt.Errorf("parse document URI: %w", err)
	}
	if parsedURI.Scheme != "file" {
		return "", fmt.Errorf("unsupported document URI scheme %q", parsedURI.Scheme)
	}
	if parsedURI.Opaque != "" || parsedURI.RawQuery != "" || parsedURI.Fragment != "" {
		return "", fmt.Errorf("unsupported file URI %q", uri)
	}

	escapedPath := parsedURI.EscapedPath()
	if escapedPath == "" {
		return "", fmt.Errorf("file URI has no path")
	}
	uriPath, err := url.PathUnescape(escapedPath)
	if err != nil {
		return "", fmt.Errorf("decode file URI path: %w", err)
	}
	if parsedURI.Host != "" && parsedURI.Host != "localhost" {
		uriPath = "//" + parsedURI.Host + uriPath
	}

	filePath := filepath.FromSlash(uriPath)
	if runtime.GOOS == "windows" && len(filePath) >= 3 && os.IsPathSeparator(filePath[0]) && filePath[2] == ':' {
		filePath = filePath[1:]
	}
	if !filepath.IsAbs(filePath) {
		return "", fmt.Errorf("file URI path %q is not absolute", uriPath)
	}

	return filepath.Clean(filePath), nil
}

func getFusionArtifactPath(projectRoot string, projectName string) (string, error) {
	if err := validateArtifactProjectName(projectName); err != nil {
		return "", err
	}

	homeDir, err := os.UserHomeDir()
	var artifactRoot string
	if err == nil {
		artifactRoot = filepath.Join(homeDir, ".dbt", "dbt-language-server", "fusion-artifacts")
	} else {
		if projectRoot == "" {
			return "", fmt.Errorf("resolve home directory for Fusion artifacts and dbt project root is empty: %w", err)
		}
		absoluteProjectRoot, pathErr := filepath.Abs(projectRoot)
		if pathErr != nil {
			return "", fmt.Errorf("resolve dbt project root %q for Fusion artifacts: %w", projectRoot, pathErr)
		}
		artifactRoot = filepath.Join(
			absoluteProjectRoot,
			"target",
			".dbt-language-server",
			"fusion-artifacts",
		)
	}

	fusionArtifactPath := filepath.Join(artifactRoot, projectName)

	if err := os.MkdirAll(fusionArtifactPath, 0755); err != nil {
		return "", fmt.Errorf("create Fusion artifact directory %q: %w", fusionArtifactPath, err)
	}

	logDir := filepath.Join(fusionArtifactPath, "log")
	if err := os.MkdirAll(logDir, 0755); err != nil {
		return "", fmt.Errorf("create Fusion log directory %q: %w", logDir, err)
	}

	targetDir := filepath.Join(fusionArtifactPath, "target")
	if err := os.MkdirAll(targetDir, 0755); err != nil {
		return "", fmt.Errorf("create Fusion target directory %q: %w", targetDir, err)
	}

	return fusionArtifactPath, nil
}

func validateArtifactProjectName(projectName string) error {
	if projectName == "" {
		return fmt.Errorf("dbt project name is empty")
	}
	if projectName == "." || projectName == ".." ||
		filepath.IsAbs(projectName) ||
		filepath.Clean(projectName) != projectName ||
		strings.ContainsAny(projectName, `/\:`) {
		return fmt.Errorf("dbt project name %q is not safe for Fusion artifact paths", projectName)
	}
	return nil
}
