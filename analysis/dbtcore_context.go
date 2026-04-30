package analysis

import (
	"fmt"
	"path/filepath"
	"strings"

	"github.com/j-clemons/dbt-language-server/lsp"
)

// buildContextFromManifest converts a dbt manifest into the same in-memory
// maps the static analysis uses. originalFilePath values in the manifest are
// relative to the package root; for the main project they resolve against
// projectRoot, for installed packages against
// <projectRoot>/<packagesInstallPath>/<packageName>.
func buildContextFromManifest(
	m *Manifest,
	projectRoot string,
	projYaml DbtProjectYaml,
) (map[string]ModelDetails, map[string]Source, map[Package]map[string]Macro) {
	mainProject := projYaml.ProjectName.Value
	packagesInstallPath := projYaml.PackagesInstallPath.Value
	if packagesInstallPath == "" {
		packagesInstallPath = "dbt_packages"
	}

	resolvePath := func(pkgName, relPath string) string {
		if pkgName == mainProject {
			return filepath.Join(projectRoot, relPath)
		}
		return filepath.Join(projectRoot, packagesInstallPath, pkgName, relPath)
	}

	resolvePatchPath := func(pkgName, patchPath string) string {
		if patchPath == "" {
			return ""
		}
		// format: "package_name://relative/path/to/schema.yml"
		if idx := strings.Index(patchPath, "://"); idx >= 0 {
			rel := patchPath[idx+3:]
			if pkgName == mainProject {
				return filepath.Join(projectRoot, rel)
			}
			return filepath.Join(projectRoot, packagesInstallPath, pkgName, rel)
		}
		return ""
	}

	modelMap := make(map[string]ModelDetails)
	sourceMap := make(map[string]Source)
	macroMap := make(map[Package]map[string]Macro)

	// Models and seeds
	for _, node := range m.Nodes {
		if node.ResourceType != "model" && node.ResourceType != "seed" {
			continue
		}

		absPath := resolvePath(node.PackageName, node.OriginalFilePath)
		schemaURI := resolvePatchPath(node.PackageName, node.PatchPath)

		name := node.Name
		if node.Config.Alias != nil && *node.Config.Alias != "" {
			name = *node.Config.Alias
		}

		modelMap[name] = ModelDetails{
			URI:         absPath,
			ProjectName: node.PackageName,
			Description: node.Description,
			SchemaURI:   schemaURI,
			SchemaRange: lsp.Range{},
		}
	}

	// Sources — grouped by source_name so the existing lookup works
	// (SourceDetailMap[sourceName].Tables[tableName])
	groupedSources := make(map[string]*Source)
	for _, src := range m.Sources {
		absPath := resolvePath(src.PackageName, src.OriginalFilePath)

		if _, ok := groupedSources[src.SourceName]; !ok {
			groupedSources[src.SourceName] = &Source{
				Name:        src.SourceName,
				Description: src.Description,
				URI:         absPath,
				Range:       lsp.Range{},
				Tables:      make(map[string]SourceTable),
			}
		}

		groupedSources[src.SourceName].Tables[src.Name] = SourceTable{
			Name:        src.Name,
			Description: src.Description,
			Table:       src.Name,
			URI:         absPath,
			Range:       lsp.Range{},
		}
	}
	for k, v := range groupedSources {
		sourceMap[k] = *v
	}

	// Macros
	for _, macro := range m.Macros {
		pkg := Package(macro.PackageName)
		if macroMap[pkg] == nil {
			macroMap[pkg] = make(map[string]Macro)
		}

		absPath := resolvePath(macro.PackageName, macro.OriginalFilePath)

		desc := macro.Description
		if len(macro.Arguments) > 0 {
			parts := make([]string, 0, len(macro.Arguments))
			for _, arg := range macro.Arguments {
				if arg.Description != "" {
					parts = append(parts, fmt.Sprintf("%s: %s", arg.Name, arg.Description))
				} else {
					parts = append(parts, arg.Name)
				}
			}
			suffix := "Arguments: " + strings.Join(parts, ", ")
			if desc != "" {
				desc += "\n\n" + suffix
			} else {
				desc = suffix
			}
		}

		macroMap[pkg][macro.Name] = Macro{
			Name:        macro.Name,
			ProjectName: pkg,
			Description: desc,
			URI:         absPath,
			Range:       lsp.Range{},
		}
	}

	return modelMap, sourceMap, macroMap
}
