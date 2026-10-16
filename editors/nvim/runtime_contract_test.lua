local function executable(path, version)
  vim.fn.mkdir(vim.fn.fnamemodify(path, ':h'), 'p')
  vim.fn.writefile({ '#!/bin/sh', "echo '" .. version .. "'" }, path)
  vim.fn.setfperm(path, 'rwxr-xr-x')
end

local root = vim.fn.tempname()
local home = root .. '/home'
local cargo_bin = root .. '/cargo-bin'
local preferred = home .. '/.local/bin/loctree-lsp'
local shadowed = cargo_bin .. '/loctree-lsp'
executable(preferred, 'loctree-lsp 0.14.1+gpreferred')
executable(shadowed, 'loctree-lsp 0.12.2')

local resolved = LoctreeRuntime.resolve({ user_home = home, path_env = cargo_bin })
local realpath = vim.uv and vim.uv.fs_realpath(preferred) or vim.loop.fs_realpath(preferred)
assert(resolved.command == realpath, 'preferred runtime path must win')
assert(resolved.source == 'preferred-install', 'preferred runtime source must be explicit')
assert(resolved.identity == 'loctree-lsp 0.14.1+gpreferred', 'active runtime identity must be probed')
assert(resolved.warning and resolved.warning:find('Loctree runtime PATH shadowing:', 1, true), 'shadow warning missing')
assert(resolved.warning:find('loctree-lsp 0.12.2', 1, true), 'shadow identity missing')
assert(vim.fn.exists(':LoctreeRuntime') == 2, 'runtime provenance command must be registered')

local path_only = LoctreeRuntime.resolve({ user_home = root .. '/empty-home', path_env = cargo_bin })
assert(path_only.command == (vim.uv and vim.uv.fs_realpath(shadowed) or vim.loop.fs_realpath(shadowed)))
assert(path_only.source == 'path', 'PATH fallback source must be explicit')
assert(path_only.identity == 'loctree-lsp 0.12.2', 'PATH runtime identity must be probed')

local configured = LoctreeRuntime.resolve({ configured = preferred, user_home = '', path_env = '' })
assert(configured.command == realpath, 'configured runtime must win')
assert(configured.source == 'configured', 'configured runtime source must be explicit')

vim.fn.delete(root, 'rf')

-- Exercise the real setup callback without replacing native navigation maps.
local original_set = vim.keymap.set
local mapped = {}
vim.keymap.set = function(_, key, _) mapped[key] = true end
LoctreeRuntime.on_attach({ server_capabilities = { codeActionProvider = true } }, 1)
assert(mapped['<leader>ca'], 'advertised code action mapping missing')
assert(not mapped.K and not mapped.gd and not mapped.gr, 'unadvertised navigation must stay native')
mapped = {}
LoctreeRuntime.on_attach({ server_capabilities = {
  hoverProvider = true, definitionProvider = true, referencesProvider = true,
} }, 1)
assert(mapped.K and mapped.gd and mapped.gr, 'advertised generic mappings missing')
assert(not mapped['<leader>ca'], 'unadvertised code actions must not be mapped')
vim.keymap.set = original_set
