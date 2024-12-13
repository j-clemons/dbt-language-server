package analysis

import (
	"os"
	"path/filepath"
)

func getPackageRootPaths(projectRoot string, projYaml DbtProjectYaml) []string {
    packagePaths := []string{}
    if projYaml.PackagesInstallPath == "" {
        return packagePaths
    }
    projectPackagePath := filepath.Join(projectRoot, projYaml.PackagesInstallPath)

    files, _ := os.ReadDir(projectPackagePath)
    for _, file := range files {
        if file.IsDir() {
            packagePaths = append(packagePaths, filepath.Join(projectPackagePath, file.Name()))
        }
    }
    return packagePaths
}

func getPackageDbtProjectYaml(packagePath string) DbtProjectYaml {
    dbtYml := parseDbtProjectYaml(packagePath)
    return dbtYml
}

func getPackageModelPaths(packagePath string) ProjectDetails {
    validModelPaths := []string{}
    dbtYml := getPackageDbtProjectYaml(packagePath)
    for _, path := range dbtYml.ModelPaths {
        _, err := os.ReadDir(filepath.Join(packagePath, path))
        if err == nil {
            validModelPaths = append(validModelPaths, path)
        }
    }
    return ProjectDetails{RootPath: packagePath, DbtProjectYaml: dbtYml}
}

func getPackageModelDetails(projectRoot string, projYaml DbtProjectYaml) []ProjectDetails {
    packagePaths := getPackageRootPaths(projectRoot, projYaml)
    packageModelPaths := []ProjectDetails{}
    for _, p := range packagePaths {
        packageModelPaths = append(packageModelPaths, getPackageModelPaths(p))
    }
    return packageModelPaths
}

func getPackageMacroPaths(packagePath string) ProjectDetails {
    validMacroPaths := []string{}
    dbtYml := getPackageDbtProjectYaml(packagePath)
    for _, path := range dbtYml.MacroPaths {
        _, err := os.ReadDir(filepath.Join(packagePath, path))
        if err == nil {
            validMacroPaths = append(validMacroPaths, path)
        }
    }
    return ProjectDetails{RootPath: packagePath, DbtProjectYaml: dbtYml}
}

func getPackageMacroDetails(projectRoot string, projYaml DbtProjectYaml) []ProjectDetails {
    packagePaths := getPackageRootPaths(projectRoot, projYaml)
    packageMacroPaths := []ProjectDetails{}
    for _, p := range packagePaths {
        packageMacroPaths = append(packageMacroPaths, getPackageMacroPaths(p))
    }
    return packageMacroPaths
}
