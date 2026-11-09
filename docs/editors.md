# Editors

How to put lsp-touchstone in front of an editor, and what `make check-editors`
(`tests/editors/run.sh`, in the `editors` dev shell) observed. The script drives each editor
without a person: connect, hover, completion and definition on `main`, type a line, run the
strict server's edit command through a code action or `workspace/executeCommand` (strict server
only), save, and shut down. The sample file has a line with `日本😀`. Each session is recorded
and summarized; the findings are about the editor and the server under test.

| Editor | Driven by |
|---|---|
| eglot | `emacs -Q --batch -l tests/editors/eglot.el` |
| lsp-mode | `emacs -Q --batch -l tests/editors/lsp-mode.el` (loads the package's autoloads as package.el does) |
| Neovim | `nvim --headless --clean -u NONE -l tests/editors/nvim.lua` |
| Helix | keystrokes in a private tmux server (`tests/editors/helix.sh`); Helix has no batch mode |
| VS Code | VSCodium under `xvfb-run`, with `tests/editors/vscode/scenario.js` running `vscode-languageclient` in the extension host (`@vscode/test-electron`) |

## Configuration

### eglot (Emacs)

```elisp
(add-to-list 'eglot-server-programs
             '(prog-mode . ("lsp-touchstone" "server" "run-server" "--web-ui-port" "4319")))

(add-to-list 'eglot-server-programs
             '(rust-ts-mode . ("lsp-touchstone" "proxy" "run-proxy" "--web-ui-port" "4318"
                               "--server-command-args" "rust-analyzer")))
```

### lsp-mode (Emacs)

```elisp
(lsp-register-client
 (make-lsp-client :new-connection (lsp-stdio-connection
                                   '("lsp-touchstone" "server" "run-server" "--web-ui-port" "4319"))
                  :major-modes '(prog-mode)
                  :priority 100
                  :server-id 'lsp-touchstone))
```

For the proxy, give `lsp-stdio-connection` the proxy command with the real server after
`--server-command-args`.

### Neovim

```lua
vim.lsp.start({
  name = "lsp-touchstone",
  cmd = { "lsp-touchstone", "server", "run-server", "--web-ui-port", "4319" },
  root_dir = vim.fs.root(0, ".git"),
})

vim.lsp.start({
  name = "rust-analyzer",
  cmd = { "lsp-touchstone", "proxy", "run-proxy", "--web-ui-port", "4318",
          "--server-command-args", "rust-analyzer" },
  root_dir = vim.fs.root(0, "Cargo.toml"),
})
```

### Helix

`.helix/languages.toml` in the workspace:

```toml
[language-server.lsp-touchstone]
command = "lsp-touchstone"
args = ["proxy", "run-proxy", "--web-ui-port", "4318", "--server-command-args", "rust-analyzer"]

[[language]]
name = "rust"
language-servers = ["lsp-touchstone"]
```

Use `args = ["server", "run-server", "--web-ui-port", "4319"]` for the strict server. `Space a`
lists its code actions.

### VS Code

An extension that starts `vscode-languageclient`'s `LanguageClient` with
`{ command: "lsp-touchstone", args: ["server", "run-server"] }` as its server options;
`tests/editors/vscode/scenario.js` is a complete example.

## Results

Emacs 31.1 with eglot 1.24.31 and lsp-mode 20261020.1728, Neovim 0.12.6, Helix 25.07.1, VSCodium
1.143.17183 with vscode-languageclient 11.0.0, rust-analyzer 2026-11-02 (all from nixpkgs). Counts
vary between runs with rust-analyzer's indexing; the table lists what appeared over several runs.

| Editor | Against | Errors | Warnings and notes |
|---|---|---|---|
| eglot | the strict server | none | `types/unexpected-params`: `exit` with `"params": {}`. `types/unknown-property`: `textDocument.$streamingDiagnostics` in `initialize`, an eglot extension |
| lsp-mode | the strict server | `jsonrpc/params-structure` in some runs: `exit` with `"params": null` | `types/unknown-property`: `completionItem.resolveAdditionalTextEditsSupport` in `initialize`, and `textDocument.version` in `didSave`. Ends the server after `shutdown` without sending `exit` |
| Neovim | the strict server | none | none |
| Helix | the strict server | none | In some runs the server ends before Helix's `exit` arrives |
| VS Code | the strict server | none | `cancel/unknown-request`: cancels a code action that was already answered |
| any | rust-analyzer | `capability/client-did-not-declare`: `workspace/diagnostic/refresh` although no client declared `workspace.diagnostics.refreshSupport`. `jsonrpc/request-never-answered`: requests still waiting when rust-analyzer answers `shutdown` and exits | `progress/before-create-response`: progress on a token before the client answered `window/workDoneProgress/create` |
| Neovim | rust-analyzer | `lifecycle/client-message-after-shutdown` in some runs: `textDocument/semanticTokens` requests after `shutdown` | |
| VS Code | rust-analyzer | `lifecycle/client-message-after-shutdown` in some runs: `$/cancelRequest` after `shutdown` | |

- The strict server offers `utf-32`, `utf-8` and `utf-16` in that order. eglot, lsp-mode,
  Neovim and Helix negotiate `utf-32`; vscode-languageclient offers only `utf-16`.
- Every editor applies the strict server's `workspace/applyEdit` (eglot, lsp-mode, Helix and VS Code
  through `documentChanges`, Neovim through `changes`), and its next `didChange` produces exactly
  the edited text.
- `workspace/configuration`: eglot answers `null` for each item, lsp-mode `{}` and `null`,
  Neovim the `settings` it was started with.
- lsp-mode sends `workspace/willRenameFiles` when Emacs renames the file to its backup
  (`main.rs~`) on the first save; rust-analyzer does not answer before lsp-mode times out and
  cancels it.
- lsp-mode answers a server request only after its handler returns; when the handler signals an
  error (here `void-function lsp-lens--enable` without the package's autoloads), the request
  gets no response at all.
- Neovim sends `shutdown` twice when `client:stop()` is followed by quitting before the client
  has stopped; the script waits for the client to go away before quitting.
