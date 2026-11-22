--- workspace/executeCommand wrappers (spec §1.6, §5.1): every dbt.* command
--- takes one argument object { uri, position? }.

local actions = require("cogitator.actions")

local M = {}

local client_for = actions.client_for

--- The single argument object of spec §1.6 (exposed for the test suite).
---@param bufnr integer
---@param with_position boolean include the cursor as a 0-based position
---@return {uri: string, position?: {line: integer, character: integer}}
function M.build_argument(bufnr, with_position)
  local arg = { uri = vim.uri_from_bufnr(bufnr) }
  if with_position then
    local pos = vim.api.nvim_win_get_cursor(0)
    arg.position = { line = pos[1] - 1, character = pos[2] }
  end
  return arg
end

local function exec(command, with_position, handler)
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = command,
    arguments = { M.build_argument(bufnr, with_position) },
  }, { bufnr = bufnr }, handler)
end

function M.run()
  exec("dbt.run", false)
end

function M.test()
  exec("dbt.test", false)
end

function M.compile()
  exec("dbt.compile", false) -- success opens the compiled SQL via showDocument
end

function M.parse()
  exec("dbt.parse", false) -- project-wide; success refreshes the server context
end

function M.source_freshness()
  exec("dbt.sourceFreshness", false) -- project-wide; sources.json feeds the lenses
end

--- :DbtPreview [limit] — `dbt show` for the current model; the result
--- table opens via showDocument when the run succeeds.
function M.preview(opts)
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  local arg = M.build_argument(bufnr, false)
  local limit = tonumber(opts and opts.args or "")
  if limit then
    arg.limit = limit
  end
  client:exec_cmd({ command = "dbt.preview", arguments = { arg } }, { bufnr = bufnr })
end

--- :DbtImpact [column] — downstream closure of this model, with
--- column-level hits when a column is given; quickfix list + summary.
function M.impact(opts)
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  local column = opts and opts.args ~= "" and opts.args or nil
  client:exec_cmd({
    command = "dbt.impact",
    arguments = { { uri = vim.uri_from_bufnr(bufnr), column = column } },
  }, { bufnr = bufnr }, function(err, result)
    if err or type(result) ~= "table" then
      vim.notify("dbt.impact failed" .. (err and (": " .. err.message) or ""), vim.log.levels.ERROR)
      return
    end
    vim.notify(result.summary, vim.log.levels.INFO)
    local items = {}
    for _, node in ipairs(result.models) do
      items[#items + 1] = {
        filename = node.uri and node.uri ~= vim.NIL and vim.uri_to_fname(node.uri) or "",
        lnum = 1,
        col = 1,
        text = ("depth %d — %s"):format(node.depth, node.name),
      }
    end
    for _, hit in ipairs(result.columnHits or {}) do
      items[#items + 1] = { text = ("column hit: %s.%s"):format(hit.model, hit.column) }
    end
    if #items > 0 then
      vim.fn.setqflist({}, " ", { title = "dbt impact: " .. result.summary, items = items })
      vim.cmd.copen()
    end
  end)
end

--- :DbtExpandJinja — render this model's Jinja (stubbed subset) into
--- a scratch split.
function M.expand_jinja()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = "dbt.expandJinja",
    arguments = { { uri = vim.uri_from_bufnr(bufnr) } },
  }, { bufnr = bufnr }, function(err, result)
    if err or type(result) ~= "table" or type(result.sql) ~= "string" then
      vim.notify(
        "dbt.expandJinja failed" .. (err and (": " .. err.message) or ""),
        vim.log.levels.ERROR
      )
      return
    end
    vim.cmd.vsplit()
    local scratch = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_lines(scratch, 0, -1, false, vim.split(result.sql, "\n"))
    vim.bo[scratch].filetype = "sql"
    vim.bo[scratch].modifiable = false
    vim.api.nvim_win_set_buf(0, scratch)
  end)
end

--- The dbt.queryPlan payload as display lines (exposed for the test suite).
---@param plan table the server's result
---@param line_of fun(span: {start: integer}): integer byte span to 1-based line
---@return string[]
function M.plan_lines(plan, line_of)
  local lines = {
    ("dbt query plan: %s (%s, %s)"):format(tostring(plan.model), plan.status, plan.backend),
  }
  if type(plan.guidance) == "string" then
    vim.list_extend(lines, vim.split(plan.guidance, "\n"))
  end
  for _, name in ipairs({ "hir", "mir", "lir" }) do
    local level = plan.levels[name]
    table.insert(lines, "")
    if level.available then
      table.insert(lines, ("== %s (%s) =="):format(name:upper(), level.fidelity))
      vim.list_extend(lines, vim.split(level.text or "", "\n"))
    else
      table.insert(lines, ("== %s unavailable: %s"):format(name:upper(), level.unavailable or "?"))
    end
    for _, gap in ipairs(level.degradations or {}) do
      table.insert(lines, ("  degraded [%s]: %s"):format(gap.kind, gap.detail))
    end
    for _, bug in ipairs(level.violations or {}) do
      table.insert(lines, ("  VIOLATION [%s]: %s"):format(bug.invariant, bug.detail))
    end
  end
  table.insert(lines, "")
  table.insert(lines, "== Findings ==")
  for _, finding in ipairs(plan.findings or {}) do
    table.insert(
      lines,
      ("%d: %s [%s] %s"):format(line_of(finding.span), finding.id, finding.severity, finding.message)
    )
  end
  table.insert(lines, "")
  table.insert(lines, ("== Rule trace%s =="):format(plan.trace.incomplete and " (incomplete)" or ""))
  for _, row in ipairs(plan.trace.entries or {}) do
    local detail = row.reason or (row.count and tostring(row.count)) or ""
    table.insert(lines, ("%-24s %-13s %-10s %s"):format(row.rule, row.phase, row.outcome, detail))
  end
  return lines
end

--- :DbtPlan — each IR level and the rule trace in a scratch split.
function M.plan()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = "dbt.queryPlan",
    arguments = { { uri = vim.uri_from_bufnr(bufnr) } },
  }, { bufnr = bufnr }, function(err, plan)
    if err or type(plan) ~= "table" or type(plan.levels) ~= "table" then
      vim.notify("dbt.queryPlan failed" .. (err and (": " .. err.message) or ""), vim.log.levels.ERROR)
      return
    end
    local lines = M.plan_lines(plan, function(span)
      return vim.api.nvim_buf_call(bufnr, function()
        return vim.fn.byte2line(span.start + 1)
      end)
    end)
    vim.cmd.vsplit()
    local scratch = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_lines(scratch, 0, -1, false, lines)
    vim.bo[scratch].filetype = "cogitatorplan"
    vim.bo[scratch].modifiable = false
    vim.api.nvim_win_set_buf(0, scratch)
  end)
end

--- :DbtDoctor — the server's health report in a float.
function M.doctor()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = "dbt.doctor",
    arguments = { { uri = vim.uri_from_bufnr(bufnr) } },
  }, { bufnr = bufnr }, function(err, report)
    if err or type(report) ~= "table" then
      vim.notify("dbt.doctor failed" .. (err and (": " .. err.message) or ""), vim.log.levels.ERROR)
      return
    end
    local lines = vim.split(vim.inspect(report), "\n")
    local ui = require("cogitator.ui")
    local width = math.min(90, vim.o.columns - 4)
    local height = math.min(#lines, vim.o.lines - 6)
    local buf, win = ui.float({
      row = math.floor((vim.o.lines - height) / 2),
      col = math.floor((vim.o.columns - width) / 2),
      width = width,
      height = height,
      title = "dbt doctor",
    })
    ui.set_lines(buf, lines)
    vim.api.nvim_set_current_win(win)
    vim.keymap.set("n", "q", vim.cmd.close, { buffer = buf, nowait = true })
  end)
end

--- :DbtDeadModels — models nothing uses, as a quickfix list.
function M.dead_models()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = "dbt.deadModels",
    arguments = { { uri = vim.uri_from_bufnr(bufnr) } },
  }, { bufnr = bufnr }, function(err, result)
    if err or type(result) ~= "table" then
      vim.notify("dbt.deadModels failed" .. (err and (": " .. err.message) or ""), vim.log.levels.ERROR)
      return
    end
    if #result.models == 0 then
      vim.notify("no dead models — everything is referenced", vim.log.levels.INFO)
      return
    end
    local items = {}
    for _, dead in ipairs(result.models) do
      items[#items + 1] = {
        filename = dead.uri and vim.uri_to_fname(dead.uri) or "",
        lnum = 1,
        col = 1,
        text = dead.name .. " — " .. table.concat(dead.reasons, "; "),
      }
    end
    vim.fn.setqflist({}, " ", { title = "dbt dead models", items = items })
    vim.cmd.copen()
  end)
end

---@type table<integer, string> tokens of dbt jobs this session started
M._jobs = {}

--- :DbtRunSelection — run dbt with an arbitrary selector (tag:nightly,
--- state:modified+, …); the returned token feeds :DbtCancel.
function M.run_selection(opts)
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  local function start(selector)
    if not selector or selector == "" then
      return
    end
    client:exec_cmd({
      command = "dbt.runSelection",
      arguments = { { uri = vim.uri_from_bufnr(bufnr), selector = selector } },
    }, { bufnr = bufnr }, function(err, result)
      if err then
        vim.notify(err.message, vim.log.levels.ERROR)
      elseif type(result) == "table" and result.token then
        M._jobs[result.token] = "dbt run: " .. selector
        vim.notify(("dbt job %d started (%s)"):format(result.token, selector), vim.log.levels.INFO)
      end
    end)
  end
  if opts and opts.args and opts.args ~= "" then
    start(opts.args)
  else
    vim.ui.input({ prompt = "dbt selector: " }, start)
  end
end

--- :DbtCancel — pick a running job this session started and cancel it.
function M.cancel()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  local tokens = vim.tbl_keys(M._jobs)
  if #tokens == 0 then
    vim.notify("no dbt jobs started from this session", vim.log.levels.INFO)
    return
  end
  table.sort(tokens)
  vim.ui.select(tokens, {
    prompt = "Cancel dbt job",
    format_item = function(token)
      return ("%d — %s"):format(token, M._jobs[token])
    end,
  }, function(token)
    if not token then
      return
    end
    client:exec_cmd({
      command = "dbt.cancel",
      arguments = { { uri = vim.uri_from_bufnr(bufnr), token = token } },
    }, { bufnr = bufnr }, function(err)
      if err then
        vim.notify(err.message, vim.log.levels.ERROR)
      else
        M._jobs[token] = nil
      end
    end)
  end)
end

--- :DbtCache [clear] — derived-cache counters, or clear + refresh.
function M.cache(action)
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  local arg = M.build_argument(bufnr, false)
  arg.action = (action == "clear") and "clear" or "stats"
  client:exec_cmd({ command = "dbt.cache", arguments = { arg } }, { bufnr = bufnr }, function(err, r)
    if err or not r or r == vim.NIL then
      vim.notify("dbt.cache failed" .. (err and (": " .. err.message) or ""), vim.log.levels.ERROR)
      return
    end
    local function line(name, surface)
      return ("%s: %d entries, %d hits, %d misses"):format(name, surface.entries, surface.hits, surface.misses)
    end
    vim.notify(table.concat({
      "cogitator derived caches" .. (arg.action == "clear" and " (cleared)" or ""),
      line("profiles", r.profiles),
      line("lineage", r.lineage),
      line("results", r.results),
    }, "\n"))
  end)
end

--- :DbtQuery — bounded SELECT on the relation under the cursor: through
--- vim-dadbod when a connection is configured, else into a scratch sql
--- buffer ready for any client.
function M.query()
  local cfg = require("cogitator").config
  local limit = (cfg and cfg.query_limit) or 100
  exec("dbt.relation", true, function(err, result)
    if err then
      vim.notify("dbt.relation failed: " .. err.message, vim.log.levels.ERROR)
      return
    end
    if not result or result == vim.NIL then
      vim.notify("no relation here — not a dbt model, ref() or source()", vim.log.levels.WARN)
      return
    end
    local statement = ("select * from %s limit %d"):format(result.relation, limit)
    if vim.fn.exists(":DB") == 2 and (vim.b.db or vim.g.db) then
      vim.cmd.DB(statement)
      return
    end
    local buf = vim.api.nvim_create_buf(true, true)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, {
      ("-- %s %s — run with vim-dadbod (:DB) or your SQL client"):format(result.kind, result.name),
      statement,
    })
    vim.bo[buf].filetype = "sql"
    vim.api.nvim_set_current_buf(buf)
    vim.notify(("query for %s (limit %d)"):format(result.relation, limit))
  end)
end

--- dbt.relation: resolve database.schema.identifier of the source()/ref()
--- under the cursor (else the file's model), yank it for pasting into SQL.
function M.relation()
  exec("dbt.relation", true, function(err, result)
    if err then
      vim.notify("dbt.relation failed: " .. err.message, vim.log.levels.ERROR)
      return
    end
    if not result or result == vim.NIL then
      vim.notify("no relation here — not a dbt model, ref() or source()", vim.log.levels.WARN)
      return
    end
    vim.fn.setreg('"', result.relation)
    pcall(vim.fn.setreg, "+", result.relation)
    vim.notify(("%s %s → %s (yanked)"):format(result.kind, result.name, result.relation))
  end)
end

function M.lineage()
  exec("dbt.showLineage", false) -- result arrives as a window/showMessage
end

function M.schema()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = "dbt.goToSchema",
    arguments = { M.build_argument(bufnr, true) },
  }, { bufnr = bufnr }, function(err, result)
    if err then
      vim.notify("dbt.goToSchema failed: " .. err.message, vim.log.levels.ERROR)
      return
    end
    if not result or result == vim.NIL or not result.uri then
      vim.notify("No schema entry found for this model.", vim.log.levels.INFO)
      return
    end
    vim.lsp.util.show_document(result, client.offset_encoding, { focus = true })
  end)
end

function M.extract()
  -- scaffold <model>.yml, or add the next version; the server applies the
  -- change via workspace/applyEdit and navigates to it
  exec("dbt.extractModelYaml", false)
end

--- Picker over every test defined for this model (and its versions).
function M.tests()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = "dbt.modelTests",
    arguments = { M.build_argument(bufnr, false) },
  }, { bufnr = bufnr }, function(err, result)
    if err or not result or result == vim.NIL or not result.tests or #result.tests == 0 then
      vim.notify(
        "No tests defined for this model. Add data_tests entries in its "
          .. "schema YAML (:DbtExtract scaffolds the entry).",
        vim.log.levels.INFO
      )
      return
    end
    vim.ui.select(result.tests, {
      prompt = ("tests for %s"):format(result.model),
      format_item = function(t)
        local label = t.name
        if t.column and t.column ~= vim.NIL then
          label = label .. " on " .. t.column
        end
        if t.version and t.version ~= vim.NIL then
          label = label .. (" (v%d)"):format(t.version)
        end
        return label
      end,
    }, function(choice)
      if choice then
        vim.lsp.util.show_document(
          { uri = choice.uri, range = choice.range },
          client.offset_encoding,
          { focus = true }
        )
      end
    end)
  end)
end

--- :DbtTags — pick a tag, then a model carrying it, and open it.
function M.tags()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = "dbt.projectGraph",
    arguments = { M.build_argument(bufnr, false) },
  }, { bufnr = bufnr }, function(err, graph)
    if err or not graph or graph == vim.NIL then
      vim.notify("dbt.projectGraph failed", vim.log.levels.ERROR)
      return
    end
    local by_tag = {}
    for _, node in ipairs(graph.nodes) do
      for _, tag in ipairs(node.tags or {}) do
        by_tag[tag] = by_tag[tag] or {}
        table.insert(by_tag[tag], node)
      end
    end
    local tags = vim.tbl_keys(by_tag)
    table.sort(tags)
    if #tags == 0 then
      vim.notify("no tags in this project (tags: in YAML or {{ config(tags=…) }})", vim.log.levels.INFO)
      return
    end
    vim.ui.select(tags, {
      prompt = "dbt tag",
      format_item = function(tag)
        return ("#%s  (%d model%s)"):format(tag, #by_tag[tag], #by_tag[tag] == 1 and "" or "s")
      end,
    }, function(tag)
      if not tag then
        return
      end
      vim.ui.select(by_tag[tag], {
        prompt = "#" .. tag,
        format_item = function(node)
          return node.name
        end,
      }, function(node)
        if node and node.uri and node.uri ~= vim.NIL then
          vim.cmd.edit(vim.uri_to_fname(node.uri))
        end
      end)
    end)
  end)
end

--- :DbtSelectors — pick a selectors.yml entry and jump to it.
function M.selectors()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:request("workspace/symbol", { query = "selector:" }, function(err, symbols)
    if err or not symbols or #symbols == 0 then
      vim.notify("no selectors.yml entries in this project", vim.log.levels.INFO)
      return
    end
    vim.ui.select(symbols, {
      prompt = "dbt selector",
      format_item = function(s)
        return s.name
      end,
    }, function(symbol)
      if symbol then
        vim.lsp.util.show_document(symbol.location, client.offset_encoding, { focus = true })
      end
    end)
  end, bufnr)
end

--- :DbtStatus — what the server knows and has been doing.
function M.status()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return
  end
  client:exec_cmd({
    command = "dbt.status",
    arguments = { M.build_argument(bufnr, false) },
  }, { bufnr = bufnr }, function(err, s)
    if err or not s or s == vim.NIL then
      vim.notify("dbt.status failed" .. (err and (": " .. err.message) or ""), vim.log.levels.ERROR)
      return
    end
    local target = s.profiles.activeTarget
    local lines = {
      ("cogitator %s — backend %s, fusion %s"):format(s.server.version, s.server.backend, s.server.fusion),
      ("project %s (%s): %d models, %d sources, %d graph edges"):format(
        s.project.name ~= vim.NIL and s.project.name or "?",
        s.project.dialect,
        s.project.models,
        s.project.sources,
        s.project.graphEdges
      ),
      ("profiles: %s / %s"):format(
        s.profiles.activeProfile ~= "" and s.profiles.activeProfile or "?",
        (target ~= vim.NIL and target ~= "") and target or "(default target)"
      ),
      ("refresh #%d in %d ms (cache %d hits / %d misses)"):format(
        s.refresh.count,
        s.refresh.lastMs,
        s.refresh.cacheHits,
        s.refresh.cacheMisses
      ),
    }
    if s.project.root ~= vim.NIL then
      table.insert(lines, "root: " .. s.project.root)
    end
    if s.config then
      if s.config.error ~= nil and s.config.error ~= vim.NIL then
        table.insert(lines, "config: BROKEN — " .. tostring(s.config.error):gsub("\n.*", ""))
      elseif s.config.loaded then
        table.insert(lines, "config: " .. tostring(s.config.path))
      else
        table.insert(lines, "config: none (cogitator.yml at the project root is hot-loaded)")
      end
    end
    vim.notify(table.concat(lines, "\n"), vim.log.levels.INFO)
  end)
end

--- Lens-aware wrapper: macro call-site lenses carry command == "" and are
--- informational only (spec §5.2).
function M.run_lens()
  local bufnr = vim.api.nvim_get_current_buf()
  local row = vim.api.nvim_win_get_cursor(0)[1] - 1
  for _, lens in ipairs(vim.lsp.codelens.get(bufnr)) do
    if lens.range.start.line == row and lens.command and lens.command.command == "" then
      vim.notify(lens.command.title .. " (informational lens)", vim.log.levels.INFO)
      return
    end
  end
  vim.lsp.codelens.run()
end

return M
