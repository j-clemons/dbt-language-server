package analysis

type Package string

func getMacroDetails(projectRoot string) map[Package]map[string]Macro {
    packageMacroMap := make(map[Package]map[string]Macro)

    dbtProjectYaml := parseDbtProjectYaml(projectRoot)
    packageDetails := getPackageMacroDetails(projectRoot, dbtProjectYaml)

    processList := []ProjectDetails{}
    processList = append(
        processList,
        ProjectDetails{
            RootPath: projectRoot,
            DbtProjectYaml: dbtProjectYaml,
        },
    )
    processList = append(processList, packageDetails...)

    for _, p := range processList {
        macros, err := parseMacros(p.RootPath, p.DbtProjectYaml)
        if err != nil {
            continue
        }

        for _, m := range macros {
            if packageMacroMap[m.ProjectName] == nil {
                packageMacroMap[m.ProjectName] = make(map[string]Macro)
            }
            packageMacroMap[m.ProjectName][m.Name] = m
        }
    }
    return packageMacroMap
}
