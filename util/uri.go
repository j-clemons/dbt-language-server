package util

import (
	"path/filepath"
	"runtime"
	"strings"
)

// PathToFileURI converts a native filesystem path to an LSP file URI.
// On Windows, backslashes are converted to forward slashes and the drive
// letter is separated correctly (file:///C:/path), matching the LSP spec.
func PathToFileURI(path string) string {
	if runtime.GOOS == "windows" {
		path = filepath.ToSlash(path)
		return "file:///" + strings.TrimPrefix(path, "/")
	}
	return "file://" + path
}

// FileURIToPath converts an LSP file URI to a native filesystem path.
// Handles the extra leading slash that Windows URIs carry (file:///C:/...).
func FileURIToPath(uri string) string {
	path := strings.TrimPrefix(uri, "file://")
	if runtime.GOOS == "windows" {
		path = strings.TrimPrefix(path, "/")
		path = filepath.FromSlash(path)
	}
	return path
}
