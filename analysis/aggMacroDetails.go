package analysis

func GetMacroDetails(projectRoot string) map[string]Macro {
    macroMap := make(map[string]Macro)

    dbtProjectYaml := ParseDbtProjectYaml(projectRoot)
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
            macroMap[m.Name] = m
        }
    }
    return macroMap
}
