package util

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
)

const releaseAPIURL = "https://api.github.com/repos/j-clemons/dbt-language-server/releases/latest"

type githubRelease struct {
	Assets []struct {
		Name               string `json:"name"`
		BrowserDownloadURL string `json:"browser_download_url"`
	} `json:"assets"`
}

// Upgrade downloads and replaces the current binary with the latest release.
// On Windows the running binary cannot be overwritten, so the new binary is
// placed alongside it as dbt-language-server-new.exe with instructions to
// replace manually.
func Upgrade() error {
	goos := runtime.GOOS
	goarch := runtime.GOARCH

	binaryName := fmt.Sprintf("dbt-language-server-%s-%s", goos, goarch)
	if goos == "windows" {
		binaryName += ".exe"
	}

	fmt.Printf("Fetching latest release for %s/%s...\n", goos, goarch)

	resp, err := http.Get(releaseAPIURL)
	if err != nil {
		return fmt.Errorf("failed to fetch release info: %w", err)
	}
	defer resp.Body.Close()

	var release githubRelease
	if err := json.NewDecoder(resp.Body).Decode(&release); err != nil {
		return fmt.Errorf("failed to parse release info: %w", err)
	}

	var downloadURL string
	for _, asset := range release.Assets {
		if asset.Name == binaryName {
			downloadURL = asset.BrowserDownloadURL
			break
		}
	}
	if downloadURL == "" {
		return fmt.Errorf("binary %s not found in the latest release", binaryName)
	}

	fmt.Printf("Downloading %s...\n", binaryName)
	dlResp, err := http.Get(downloadURL)
	if err != nil {
		return fmt.Errorf("failed to download binary: %w", err)
	}
	defer dlResp.Body.Close()

	exePath, err := os.Executable()
	if err != nil {
		return fmt.Errorf("failed to determine executable path: %w", err)
	}
	exePath, err = filepath.EvalSymlinks(exePath)
	if err != nil {
		return fmt.Errorf("failed to resolve symlinks: %w", err)
	}

	dir := filepath.Dir(exePath)

	if goos == "windows" {
		// Windows locks the running executable, so write alongside it and ask
		// the user to replace it after the process exits.
		newPath := filepath.Join(dir, "dbt-language-server-new.exe")
		f, err := os.Create(newPath)
		if err != nil {
			return fmt.Errorf("failed to create download target: %w", err)
		}
		_, err = io.Copy(f, dlResp.Body)
		f.Close()
		if err != nil {
			os.Remove(newPath)
			return fmt.Errorf("failed to write download: %w", err)
		}
		fmt.Printf("\nDownload complete: %s\n", newPath)
		fmt.Printf("To finish upgrading, replace %s with %s\n", exePath, newPath)
		return nil
	}

	// Unix: write to a temp file in the same directory, then atomically rename.
	tmp, err := os.CreateTemp(dir, "dbt-language-server-upgrade-*")
	if err != nil {
		return fmt.Errorf("failed to create temp file: %w", err)
	}
	tmpPath := tmp.Name()

	_, err = io.Copy(tmp, dlResp.Body)
	tmp.Close()
	if err != nil {
		os.Remove(tmpPath)
		return fmt.Errorf("failed to write download: %w", err)
	}

	if err := os.Chmod(tmpPath, 0755); err != nil {
		os.Remove(tmpPath)
		return fmt.Errorf("failed to set permissions: %w", err)
	}

	if err := os.Rename(tmpPath, exePath); err != nil {
		os.Remove(tmpPath)
		return fmt.Errorf("failed to replace binary: %w", err)
	}

	fmt.Println("Upgrade complete!")
	return nil
}
