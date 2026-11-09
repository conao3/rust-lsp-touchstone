-- Drive Neovim's LSP client against lsp-touchstone in headless mode. TOUCHSTONE_COMMAND is the
-- language server command as a JSON array, TOUCHSTONE_FILE the file to edit, and TOUCHSTONE_MODE
-- "server" (the strict server) or "proxy" (a real server behind the proxy).

local command = vim.json.decode(os.getenv("TOUCHSTONE_COMMAND"))
local file = os.getenv("TOUCHSTONE_FILE")
local server_mode = os.getenv("TOUCHSTONE_MODE") == "server"

vim.cmd.edit(file)
local buf = vim.api.nvim_get_current_buf()
local client_id = vim.lsp.start({
  name = "lsp-touchstone",
  cmd = command,
  root_dir = vim.fs.dirname(file),
  settings = { ["lsp-touchstone"] = { answer = 42 } },
})
assert(vim.wait(10000, function()
  local c = vim.lsp.get_client_by_id(client_id)
  return c ~= nil and c.initialized
end), "the client did not initialize")
local client = vim.lsp.get_client_by_id(client_id)
vim.wait(2000)

local line = vim.fn.search("main")
vim.api.nvim_win_set_cursor(0, { line, vim.fn.col(".") + 1 })
local params = vim.lsp.util.make_position_params(0, client.offset_encoding)
client:request_sync("textDocument/hover", params, 5000, buf)
client:request_sync("textDocument/completion", params, 5000, buf)
client:request_sync("textDocument/definition", params, 5000, buf)

vim.api.nvim_buf_set_lines(buf, -1, -1, false, { "// typed in neovim 😀" })
vim.wait(500)
if server_mode then
  client:request_sync("workspace/executeCommand", {
    command = "lsp-touchstone.applyEdit",
    arguments = { vim.uri_from_bufnr(buf) },
  }, 5000, buf)
  vim.wait(1000)
end
vim.cmd.write()
vim.wait(1000)
client:stop()
assert(vim.wait(10000, function() return vim.lsp.get_client_by_id(client_id) == nil end), "the client did not stop")
vim.cmd("qa!")
