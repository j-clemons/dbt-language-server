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
	path := startPath
	for {
		files, err := os.ReadDir(path)
		if err != nil {
			return "", err
		}

		for _, file := range files {
			if file.Name() == fileName {
				return path, nil
			}
		}
		if path == "/" {
			return "", fmt.Errorf("File %s not found", fileName)
		}

		path = filepath.Dir(path)
	}
}

func findFileDirDesc(fileName string, startPath string) (string, error) {
	dirQueue := []string{startPath}
	for len(dirQueue) > 0 {
		path := dirQueue[0]
		dirQueue = dirQueue[1:]

		dirEntries, err := os.ReadDir(path)
		if err != nil {
			return "", err
		}

		for _, entry := range dirEntries {
			if entry.Name() == fileName {
				return path, nil
			} else if entry.IsDir() {
				dirQueue = append(dirQueue, filepath.Join(path, entry.Name()))
			}
		}
	}

	return "", fmt.Errorf("fileName not found in descending search.")
}

func GetProjectRoot(projFile string, wd string) string {
	if wd == "" {
		wd, _ = os.Getwd()
	}
	dir, err := findFileDir(projFile, wd)
	if err != nil {
		log.Print(err)
		return ""
	}

	return dir
}
