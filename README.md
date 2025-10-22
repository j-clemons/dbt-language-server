# dbt Language Server

LSP for dbt

## Features

- **Code Completion**
- **Hover Information**
- **Go to Definition**
- **[Go to Schema](analysis/README.md)**
- **Function Documentation**

| Resource | Go to Definition | Hover | Completion |
| --- | --- | --- | --- |
| Model References | x | x | x |
| Sources | x | x | x |
| Seeds | x | x | x |
| Macros | x | x | x |
| Variables | x | x | x |
| Functions |   | x | x |

### Function Documentation
This is the only part of the LSP that is dialect specific. The rest is parsed 
using the file system and a very forgiving parser that is primarily focused on 
dbt specific syntax instead of attempting to be a full SQL parser.

Supported Dialects:
- Snowflake
- BigQuery

### dbt Fusion Static Analysis
If you have dbt fusion installed, you can use it for static analysis and the 
results from compilation will be returned as diagnostics in the editor.
All artifacts from the compilation will be written to a separate directory from 
the project you are editing.

Enabled via a cli argument.
```
-f, --fusion=[path]
```
If path to the dbt fusion executable is not provided, `dbt` will be used and will look for it in `$PATH`.

## Installation

### Neovim

Add executable to $PATH
Configure with your LSP client (e.g., nvim-lspconfig):

```lua
require'lspconfig'.dbt.setup{
  cmd = { "dbt-language-server" },
  filetypes = { "sql", "yaml" },
  root_dir = require'lspconfig'.util.root_pattern("dbt_project.yml"),
}
```

### Helix

Add executable to $PATH and add to languages.toml

```toml
[language-server.dbt-language-server]
command = "dbt-language-server"

[[language]]
name = "dbt"
scope = "dbt_project.yml"
file-types = ["sql","yml","yaml"]
language-servers = ["dbt-language-server"]
```
