# Agent Guidelines for dbt-lsp

## Build/Test Commands
- **Build**: `go build .` or `go build -o dbt-language-server .`
- **Test all**: `go test ./...`
- **Test single package**: `go test ./analysis` or `go test ./lsp`
- **Test single file**: `go test -run TestFunctionName ./package`
- **Cross-platform build**: `./build-all-platforms.sh`

## Code Style
- **Imports**: Standard library first, then external packages, then local packages with blank lines between groups
- **Naming**: Use camelCase for unexported, PascalCase for exported. Struct fields are PascalCase
- **Types**: Explicit struct field tags for JSON (`json:"fieldName"`)
- **Error handling**: Always check errors with `if err != nil` pattern, log with descriptive context
- **Logging**: Use `util.GetLogger()` for consistent logging, prefix messages with context
- **Structs**: Group related fields, use composition over inheritance
- **Functions**: Keep functions focused, use receiver methods for type-specific operations
- **Indentation**: Use 4 spaces for indentation

## Project Structure
- `/analysis`: Core parsing and state management
- `/lsp`: Language Server Protocol implementation  
- `/rpc`: RPC message handling
- `/util`: Shared utilities
- `/testdata`: Test fixtures and sample dbt projects
