# Editors

How to put lsp-touchstone in front of an editor, and what `make check-editors`
(`tests/editors/run.sh`) observed. The script drives each editor in batch: connect, hover,
completion and definition on `main`, type a line, run the strict server's edit command (strict
server only), save, and shut down. The sample file has a line with `日本😀`.

## eglot (Emacs)

```elisp
;; The strict server for any programming mode.
(add-to-list 'eglot-server-programs
             '(prog-mode . ("lsp-touchstone" "server" "run-server" "--web-ui-port" "4319")))

;; A real server behind the proxy.
(add-to-list 'eglot-server-programs
             '(rust-ts-mode . ("lsp-touchstone" "proxy" "run-proxy" "--web-ui-port" "4318"
                               "--server-command-args" "rust-analyzer")))
```

`M-x eglot-code-actions` lists the strict server's commands. `M-x eglot-events-buffer` shows the
same messages as the web UI.

## Neovim

```lua
vim.lsp.start({
  name = "lsp-touchstone",
  cmd = { "lsp-touchstone", "server", "run-server", "--web-ui-port", "4319" },
  root_dir = vim.fs.root(0, ".git"),
})

-- A real server behind the proxy.
vim.lsp.start({
  name = "rust-analyzer",
  cmd = { "lsp-touchstone", "proxy", "run-proxy", "--web-ui-port", "4318",
          "--server-command-args", "rust-analyzer" },
  root_dir = vim.fs.root(0, "Cargo.toml"),
})
```

`vim.lsp.buf.code_action()` lists the strict server's commands.

## Results

Emacs 31.1 with eglot 1.24.31 and Neovim 0.12.6 (nixpkgs), rust-analyzer from nixpkgs
(2026-11-02).

| Editor | Against | Errors | Warnings |
|---|---|---|---|
| eglot | the strict server | 0 | `types/unexpected-params` on `exit` (`"params": {}`), `types/unknown-property` on `initialize` (`textDocument.$streamingDiagnostics`, an eglot extension) |
| eglot | rust-analyzer | `capability/client-did-not-declare` when rust-analyzer sends `workspace/diagnostic/refresh` (not in every run) | `progress/before-create-response` from rust-analyzer, and eglot's two warnings above |
| Neovim | the strict server | 0 | 0 |
| Neovim | rust-analyzer | 0 | `progress/before-create-response` from rust-analyzer |

- Both editors negotiate `utf-32` with the strict server, apply its `workspace/applyEdit`
  (eglot through `documentChanges`, Neovim through `changes`), and their next `didChange`
  produces exactly the edited text.
- eglot answers `workspace/configuration` with `null` for each item; Neovim answers with the
  `settings` it was started with.
- rust-analyzer sends `workspace/diagnostic/refresh` although the client did not declare
  `workspace.diagnostics.refreshSupport`, and reports progress on a token before the client has
  answered `window/workDoneProgress/create`.
- Calling Neovim's `client:stop()` and quitting before the client has stopped sends `shutdown`
  twice; the script waits for the client to go away before quitting.
