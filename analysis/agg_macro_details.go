package analysis

type Package string

func (s *State) getMacroDetails() map[Package]map[string]Macro {
	packageMacroMap := make(map[Package]map[string]Macro)
	packageDetails := getPackageMacroDetails(s.DbtContext.ProjectRoot, s.DbtContext.ProjectYaml)

	processList := []ProjectDetails{}
	processList = append(
		processList,
		ProjectDetails{
			RootPath:       s.DbtContext.ProjectRoot,
			DbtProjectYaml: s.DbtContext.ProjectYaml,
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
