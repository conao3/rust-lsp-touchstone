# Coding agents

The LSP clients of coding agents, scored against the strict server with `check check-client`.
The configurations are in [tests/agents/](../tests/agents/). Each agent starts
`lsp-touchstone server run-server` for files ending in `.tch` and opens
[tests/agents/sample.tch](../tests/agents/sample.tch).

The strict server exercises a feature only when the client declared the capability for it, so a
rule with no finding may not have been exercised. The tables list the rules with a finding and
the features the session exercised.

## Results

opencode 1.19.5 (nixpkgs) and Claude Code 2.1.319, each run three times with the same outcome.

| Rule | opencode `debug lsp diagnostics` | opencode `debug lsp document-symbols` | Claude Code (`claude -p`) |
|---|---|---|---|
| `types/unknown-property` | warned (2): `initialize` declares `textDocument.synchronization.didOpen` and `didChange`, which the specification does not define | warned (2): the same | no finding |
| `capability/server-did-not-declare` | no finding | violated (1): `textDocument/documentSymbol` without `documentSymbolProvider` | violated (1): the same |
| `jsonrpc/request-never-answered` | no finding | violated (1): the client ends the server without answering `workspace/configuration` | no finding |
| `lifecycle/shutdown-not-sent` | warned (1): the client ends the server without `shutdown` | warned (1): the same | no finding |
| `lifecycle/exit-not-sent` | no finding | no finding | warned (1): `shutdown` is answered, then the server is ended before an `exit` arrives |

What each session exercised:

| Feature | opencode | Claude Code |
|---|---|---|
| Declared client capabilities | `window.workDoneProgress`, `workspace.configuration`, `workspace.didChangeWatchedFiles.dynamicRegistration`, `textDocument.diagnostic` | `textDocument` hover, definition, references, documentSymbol, callHierarchy, `didSave`; `workspace.configuration` and dynamic registration declared `false`; no `window.workDoneProgress` |
| Progress (`create`, `begin`, `report`, `end`) | answered `create`; no finding | not declared, not exercised |
| `client/registerCapability`, `workspace/configuration` | answered (diagnostics run) | not declared, not exercised |
| `didOpen`, `didChange`, `didSave` | `didOpen` (diagnostics run) | `didOpen`; `didChange` with the full text and version 2 after an edit; `didSave` without text; no finding |
| Requests | `textDocument/documentSymbol` | `textDocument/documentSymbol`, `textDocument/hover` |

Claude Code's debug log (`--debug-file`) shows `Sending notification 'exit'` and `LSP client
stopped` in the same millisecond in all three runs, and the strict server never read the
`exit`. Claude Code answers no server-to-client request here because it declares none of the
capabilities the strict server needs to send one.

## How to run

Build lsp-touchstone and put it on `PATH` (`nix build`, then `export PATH=$PWD/result/bin:$PATH`).

### opencode

[tests/agents/opencode.json](../tests/agents/opencode.json) adds the strict server to
opencode's `lsp` configuration. `opencode debug lsp` drives the LSP client without a model.

```console
$ cd tests/agents
$ lsp-touchstone --output table check check-client --record-file /tmp/opencode-diagnostics.ndjson --client-command-args opencode debug lsp diagnostics sample.tch
$ lsp-touchstone --output table check check-client --record-file /tmp/opencode-symbols.ndjson --client-command-args opencode debug lsp document-symbols "file://$PWD/sample.tch"
```

`.github/workflows/action.yml` runs both in CI.

### Claude Code

[tests/agents/claude-plugin/](../tests/agents/claude-plugin/) is a plugin whose `.lsp.json`
declares the strict server; `--plugin-dir` loads it for one run. Claude Code has no command that
drives its LSP client without a model, so the session is a prompt that makes the model call the
LSP tool and edit the file. It needs a logged-in Claude Code and spends a few model calls.

```console
$ cd tests/agents
$ lsp-touchstone --output table check check-client --record-file /tmp/claude-code.ndjson --client-command-args claude -p --model haiku --plugin-dir claude-plugin --allowedTools LSP,Read,Edit --permission-mode acceptEdits "In sample.tch: 1) use the LSP tool to get the document symbols of sample.tch, 2) use the LSP tool to hover at line 2 character 9, 3) use the Edit tool to change the word main to start."
$ git checkout sample.tch
```

`claude --debug-file <file>` logs each LSP message the client sends and receives.
