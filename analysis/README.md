# Analysis

## Commands

### `dbt.goToSchema`

Navigate to the schema definition for a dbt model.

**Command**: `dbt.goToSchema`

**Parameters**:
```typescript
{
  uri: string;      // URI of the current document
  position: {       // Cursor position in the document
    line: number;     // Zero-based line number
    character: number; // Zero-based character offset
  };
}
```

**Behavior**:
1. **When cursor is on a `ref()` token**: Navigates to the schema definition of the referenced model
2. **When cursor is elsewhere**: Navigates to the schema definition of the current model file

**Returns**:
- `Location` object with URI and range of the schema definition, or `null` if no schema is found

**Example Usage**:
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "workspace/executeCommand",
  "params": {
    "command": "dbt.goToSchema",
    "arguments": [
      {
        "uri": "file:///path/to/models/customers.sql",
        "position": {
          "line": 5,
          "character": 15
        }
      }
    ]
  }
}
```

**Example function for implementing in Neovim**
```lua  
-- Setup dbt-lsp with proper on_attach
lspconfig.dbt_lsp.setup({
    capabilities = capabilities,
    on_attach = function(client, bufnr)
        -- dbt-specific keybinding for schema navigation
        vim.keymap.set('n', '<leader>ds', function()
            local params = {
                command = 'dbt.goToSchema',
                arguments = {
                    {
                        uri = vim.uri_from_bufnr(bufnr),
                        position = {
                            line = vim.fn.line('.') - 1,  -- LSP uses 0-based indexing
                            character = vim.fn.col('.') - 1
                        }
                    }
                }
            }

            -- Execute command and handle response
            client.request('workspace/executeCommand', params, function(err, result)
                if err then
                    vim.notify('Error executing dbt.goToSchema: ' .. tostring(err), vim.log.levels.ERROR)
                    return
                end

                if not result then
                    vim.notify('No schema definition found', vim.log.levels.WARN)
                    return
                end

                -- Navigate to the location
                local location = result
                if location.uri and location.range then
                    -- Convert file:// URI to local path
                    local file_path = vim.uri_to_fname(location.uri)

                    -- Open the file
                    vim.cmd('edit ' .. vim.fn.fnameescape(file_path))

                    -- Navigate to the specific line and column
                    local line = location.range.start.line + 1  -- Convert back to 1-based indexing
                    local col = location.range.start.character + 1
                    vim.fn.cursor(line, col)

                    vim.notify('Navigated to schema definition')
                else
                    vim.notify('Invalid location response', vim.log.levels.WARN)
                end
            end, bufnr)
        end, { buffer = bufnr, desc = 'Go to dbt schema definition' })
    end,
})
```
