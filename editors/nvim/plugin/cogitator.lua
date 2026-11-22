-- User commands (spec §5.1). Defined globally; each errors cleanly when no
-- cogitator client is attached to the current buffer.

if vim.g.loaded_cogitator then
  return
end
vim.g.loaded_cogitator = true

local function cmd(name, fn, desc)
  vim.api.nvim_create_user_command(name, function()
    require("cogitator.commands")[fn]()
  end, { desc = desc })
end

cmd("DbtRun", "run", "dbt run for the current model")
cmd("DbtTest", "test", "dbt test for the current model")
cmd("DbtCompile", "compile", "dbt compile for the current model (opens compiled SQL)")
cmd("DbtParse", "parse", "dbt parse for the whole project (refreshes artifacts + context)")
cmd(
  "DbtSourceFreshness",
  "source_freshness",
  "dbt source freshness for the project (feeds the freshness lenses)"
)
vim.api.nvim_create_user_command("DbtRunSelection", function(opts)
  require("cogitator.commands").run_selection(opts)
end, { nargs = "?", desc = "dbt run with an arbitrary selector (tag:…, state:modified+)" })
vim.api.nvim_create_user_command("DbtPreview", function(opts)
  require("cogitator.commands").preview(opts)
end, { nargs = "?", desc = "dbt show for the current model (optional row limit, default 50)" })
cmd("DbtExpandJinja", "expand_jinja", "render this model's Jinja (stubbed subset) in a split")
cmd("DbtPlan", "plan", "this model's IR levels and rule trace (dbt.queryPlan)")
cmd("DbtDoctor", "doctor", "the server's health report (dbt.doctor)")
cmd("DbtDeadModels", "dead_models", "list models nothing downstream uses (quickfix)")
vim.api.nvim_create_user_command("DbtImpact", function(opts)
  require("cogitator.commands").impact(opts)
end, { nargs = "?", desc = "downstream impact of this model (optionally of one column)" })
vim.api.nvim_create_user_command("DbtCancel", function()
  require("cogitator.commands").cancel()
end, { desc = "cancel a running dbt job started from this session" })
cmd("DbtRelation", "relation", "resolve database.schema.identifier under the cursor (yanks it)")
cmd("DbtQuery", "query", "bounded SELECT on the relation under the cursor (dadbod or scratch)")
cmd("DbtSchema", "schema", "jump to the model's properties-YAML entry")
cmd("DbtLineage", "lineage", "show column lineage for the current model")
cmd("DbtExtract", "extract", "extract the model into its schema YAML (or add a version)")
cmd("DbtTests", "tests", "pick a test defined for this model/version and jump to it")
cmd("DbtStatus", "status", "show what the language server knows and has been doing")
cmd("DbtTags", "tags", "pick a dbt tag, then a model carrying it")
cmd("DbtSelectors", "selectors", "pick a selectors.yml entry and jump to it")

vim.api.nvim_create_user_command("DbtCache", function(opts)
  require("cogitator.commands").cache(opts.args)
end, {
  nargs = "?",
  complete = function()
    return { "clear" }
  end,
  desc = "derived-cache counters (argument 'clear' empties them)",
})

vim.api.nvim_create_user_command("DbtProjects", function()
  require("cogitator.manager").pick_project()
end, { desc = "pick a known dbt project (manager store)" })

vim.api.nvim_create_user_command("DbtExplorer", function()
  require("cogitator.explorer").open()
end, { desc = "project-wide model explorer (alias of :DbtBrowse)" })

vim.api.nvim_create_user_command("DbtDag", function()
  require("cogitator.dag").open()
end, { desc = "project DAG with columns, dbt-docs style (Neovim only)" })

vim.api.nvim_create_user_command("DbtGraph", function()
  require("cogitator.graph").open()
end, { desc = "browse the model graph (upstream | model | downstream)" })

vim.api.nvim_create_user_command("DbtBrowse", function()
  require("cogitator.browse").open()
end, { desc = "project-wide model browser: search, lineage, SQL, compiled, versions, tests (Neovim only)" })

vim.api.nvim_create_user_command("DbtTarget", function()
  require("cogitator.profiles").pick_target()
end, { desc = "pick the active dbt profile/target (auth-aware)" })

vim.api.nvim_create_user_command("DbtProfiles", function(opts)
  local profiles = require("cogitator.profiles")
  if opts.args == "" then
    profiles.report(require("cogitator").config or require("cogitator.config").build({}))
  else
    profiles.set(opts.args)
  end
end, {
  nargs = "?",
  complete = "dir",
  desc = "show the dbt profiles resolution, or switch the profiles directory",
})
