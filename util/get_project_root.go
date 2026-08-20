package util

import (
	"fmt"
	"log"
	"os"
	"path/filepath"
)

func findFileDir(fileName string, startPath string) (string, error) {
	ascPath, ascErr := findFileDirAsc(fileName, startPath)
	if ascErr == nil {
		return ascPath, nil
	}

	descPath, descErr := findFileDirDesc(fileName, startPath)
	if descErr != nil {
		return "", descErr
	}

	return descPath, nil
}

func findFileDirAsc(fileName string, startPath string) (string, error) {
	path, err := filepath.Abs(startPath)
	if err != nil {
		return "", fmt.Errorf("resolve search path %q: %w", startPath, err)
	}

	for {
		files, err := os.ReadDir(path)
		if err != nil {
			return "", fmt.Errorf("read directory %q while searching for %q: %w", path, fileName, err)
		}

		for _, file := range files {
			if file.Name() == fileName {
				return path, nil
			}
		}

		parent := filepath.Dir(path)
		if parent == path {
			return "", fmt.Errorf("file %q not found from %q", fileName, startPath)
		}

		path = parent
	}
}

func findFileDirDesc(fileName string, startPath string) (string, error) {
	dirQueue := []string{startPath}
	for len(dirQueue) > 0 {
		path := dirQueue[0]
		dirQueue = dirQueue[1:]

		dirEntries, err := os.ReadDir(path)
		if err != nil {
			return "", fmt.Errorf("read directory %q while searching for %q: %w", path, fileName, err)
		}

		for _, entry := range dirEntries {
			if entry.Name() == fileName {
				return path, nil
			} else if entry.IsDir() {
				dirQueue = append(dirQueue, filepath.Join(path, entry.Name()))
			}
		}
	}

	return "", fmt.Errorf("file %q not found below %q", fileName, startPath)
}

func GetProjectRoot(projFile string, wd string) string {
	if wd == "" {
		var err error
		wd, err = os.Getwd()
		if err != nil {
			log.Printf("Failed to get working directory while searching for %q: %v", projFile, err)
			return ""
		}
	}
	dir, err := findFileDir(projFile, wd)
	if err != nil {
		log.Print(err)
		return ""
	}

	return dir
}
