-- Loctree LSP configuration for Neovim
--
-- Add this to your Neovim config (e.g., ~/.config/nvim/lua/plugins/loctree.lua)
--
-- 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

-- Runtime contract shared with the VS Code and JetBrains plugins:
--   1. valid user override (`vim.g.loctree_lsp_path`, file or directory)
--   2. preferred user install (`~/.local/bin/loctree-lsp`)
--   3. exact executable resolved from PATH
--
-- The resolved path, source, and `--version` identity are available through
-- `:LoctreeRuntime` and `_G.LoctreeRuntime.current`. A stale Cargo/Homebrew
-- entry earlier on PATH is reported, but never wins over ~/.local/bin.
local PATH_SHADOW_WARNING = 'Loctree runtime PATH shadowing:'
local BINARY_NAME = vim.fn.has('win32') == 1 and 'loctree-lsp.exe' or 'loctree-lsp'
local PATH_SEPARATOR = package.config:sub(3, 3)

local function canonical_executable(candidate)
  if not candidate or candidate == '' or vim.fn.executable(candidate) ~= 1 then
    return nil
  end
  local real = vim.uv and vim.uv.fs_realpath(candidate) or vim.loop.fs_realpath(candidate)
  return real or vim.fn.fnamemodify(candidate, ':p')
end

local function configured_executable(raw)
  if not raw or vim.trim(raw) == '' then
    return nil
  end
  local candidate = vim.fn.expand(vim.trim(raw))
  if vim.fn.isdirectory(candidate) == 1 then
    candidate = candidate .. '/' .. BINARY_NAME
  end
  return canonical_executable(candidate)
end

local function runtime_identity(command)
  local output = vim.fn.system({ command, '--version' })
  if vim.v.shell_error ~= 0 then
    return 'version unavailable'
  end
  return output:match('([^\r\n]+)') or 'version unavailable'
end

local function resolve_loctree_runtime(options)
  options = options or {}
  local configured = configured_executable(options.configured or vim.g.loctree_lsp_path)
  if configured then
    return { command = configured, source = 'configured', identity = runtime_identity(configured) }
  end

  local home = options.user_home or vim.env.HOME or ''
  local preferred = canonical_executable(home .. '/.local/bin/' .. BINARY_NAME)
  local path_match
  if options.path_env then
    for entry in string.gmatch(options.path_env, '([^' .. PATH_SEPARATOR .. ']+)') do
      path_match = canonical_executable(entry .. '/' .. BINARY_NAME)
      if path_match then break end
    end
  else
    path_match = canonical_executable(vim.fn.exepath(BINARY_NAME))
  end

  if preferred then
    local identity = runtime_identity(preferred)
    local warning
    if path_match and path_match ~= preferred then
      warning = string.format(
        '%s %s (%s) appears first on PATH, but the preferred install %s (%s) will be used. Remove or reorder the stale entry.',
        PATH_SHADOW_WARNING,
        path_match,
        runtime_identity(path_match),
        preferred,
        identity
      )
    end
    return { command = preferred, source = 'preferred-install', identity = identity, warning = warning }
  end

  local command = path_match or BINARY_NAME
  return { command = command, source = 'path', identity = runtime_identity(command) }
end

local runtime = resolve_loctree_runtime()
_G.LoctreeRuntime = { current = runtime, resolve = resolve_loctree_runtime }

if runtime.warning then
  vim.schedule(function()
    vim.notify(runtime.warning, vim.log.levels.WARN, { title = 'Loctree' })
  end)
end

vim.api.nvim_create_user_command('LoctreeRuntime', function()
  local lines = {
    'Binary: ' .. runtime.command,
    'Identity: ' .. runtime.identity,
    'Source: ' .. runtime.source,
  }
  if runtime.warning then table.insert(lines, 'Warning: ' .. runtime.warning) end
  vim.notify(table.concat(lines, '\n'), vim.log.levels.INFO, { title = 'Loctree runtime' })
end, { desc = 'Show active loctree-lsp runtime provenance', force = true })

-- Option 1: Using nvim-lspconfig (recommended)
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

-- Register loctree as a custom LSP server
if not configs.loctree then
  configs.loctree = {
    default_config = {
      cmd = { runtime.command },
      filetypes = { 'typescript', 'typescriptreact', 'javascript', 'javascriptreact', 'rust', 'python', 'go' },
      root_dir = function(fname)
        return lspconfig.util.root_pattern('.loctree', '.git')(fname)
      end,
      settings = {},
    },
  }
end

-- Setup with your preferred on_attach and capabilities
local function loctree_on_attach(client, bufnr)
  -- Diagnostics are pushed; code actions are advertised by loctree-lsp.
  -- Hover/navigation/references belong to the native language server.
  -- Generic mappings below are installed only when this client advertises them.
  local opts = { noremap = true, silent = true, buffer = bufnr }

  local caps = client.server_capabilities or {}
  if caps.hoverProvider then
    vim.keymap.set('n', 'K', vim.lsp.buf.hover, opts)
  end
  if caps.definitionProvider then
    vim.keymap.set('n', 'gd', vim.lsp.buf.definition, opts)
  end
  if caps.referencesProvider then
    vim.keymap.set('n', 'gr', vim.lsp.buf.references, opts)
  end
  if caps.codeActionProvider then
    vim.keymap.set('n', '<leader>ca', vim.lsp.buf.code_action, opts)
  end

  -- Loctree-specific commands
  vim.keymap.set('n', '<leader>lr', ':!loct<CR>', { desc = 'Loctree: Refresh' })
  vim.keymap.set('n', '<leader>lh', ':!loct health<CR>', { desc = 'Loctree: Health' })
  vim.keymap.set('n', '<leader>li', function()
    local file = vim.fn.expand('%:.')
    vim.cmd('!loct impact "' .. file .. '"')
  end, { desc = 'Loctree: Impact' })
end
LoctreeRuntime.on_attach = loctree_on_attach
lspconfig.loctree.setup {
  on_attach = loctree_on_attach,
  capabilities = vim.lsp.protocol.make_client_capabilities(),
}

-- Option 2: Manual LSP setup (if not using lspconfig)
--[[
vim.api.nvim_create_autocmd('FileType', {
  pattern = { 'typescript', 'typescriptreact', 'javascript', 'javascriptreact', 'rust' },
  callback = function()
    vim.lsp.start({
      name = 'loctree',
      cmd = { runtime.command },
      root_dir = vim.fs.dirname(vim.fs.find({ '.loctree', '.git' }, { upward = true })[1]),
    })
  end,
})
]]

-- Diagnostic signs (optional customization; overrides global Neovim diagnostic signs)
vim.api.nvim_set_hl(0, 'LoctreeAmber', { fg = '#c99a3b' })
vim.api.nvim_set_hl(0, 'LoctreeTeal', { fg = '#3d7a72' })
vim.api.nvim_set_hl(0, 'LoctreeDanger', { fg = '#b86a5c' })

vim.fn.sign_define('DiagnosticSignWarn', { text = '⚠', texthl = 'LoctreeAmber' })
vim.fn.sign_define('DiagnosticSignInfo', { text = '●', texthl = 'LoctreeTeal' })
vim.fn.sign_define('DiagnosticSignError', { text = '✖', texthl = 'LoctreeDanger' })
vim.fn.sign_define('DiagnosticSignHint', { text = '◌', texthl = 'LoctreeTeal' })

-- Status line integration (for lualine or similar)
-- Shows loctree diagnostic count
local function loctree_status()
  local diagnostics = vim.diagnostic.get(0, { source = 'loctree' })
  if #diagnostics == 0 then
    return '%#LoctreeTeal#🌳 healthy%*'
  end
  return '%#LoctreeDanger#🌳 ' .. #diagnostics .. ' issues%*'
end

-- 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders

-- Add to your lualine config:
-- sections = { lualine_x = { loctree_status, 'encoding', 'fileformat' } }
