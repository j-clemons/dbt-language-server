# dbt Language Server

LSP for dbt core

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

## Installation

### Neovim

Configure with your LSP client (e.g., nvim-lspconfig):

```lua
require'lspconfig'.dbt.setup{
  cmd = { "/path/to/dbt-language-server" },
  filetypes = { "sql", "yaml" },
  root_dir = require'lspconfig'.util.root_pattern("dbt_project.yml"),
}
```
