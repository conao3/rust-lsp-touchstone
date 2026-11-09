# AGENTS.md

`lsp-touchstone` is a Rust CLI that checks Language Server Protocol implementations: a checking
proxy, a strict client and a strict server sharing one set of rules.

## Layout

- `src/main.rs`: the command definitions (clap derive, AWS CLI grammar), `--output` rendering
  and exit codes 252/254/255.
- `src/finding.rs`: a finding (severity, code, message, byte offset).
- `src/framing.rs`: the base protocol framing reader and its checks.
- `src/jsonrpc.rs`: classifying a message as a request, response or notification, with its checks.
- `src/monitor.rs`: the message log of one session and every check on it: pairing, lifecycle,
  types, capabilities, the mirror of open documents, positions from the server, cancellation and
  progress. Every mode sends every message it reads or writes through `Monitor::observe` before
  forwarding it, so a response is never logged before its request.
- `src/model.rs`: the embedded meta model, its message definitions and capability bindings, and
  the type validator.
- `src/document.rs`: lines and positions in a document's text.
- `src/progress.rs`: the progress token state machine, written and proved in a `lean!` block.
- `src/session.rs`: the strict client: the server process, sending, and automatic answers to
  server requests.
- `src/proxy.rs`: the checking proxy (`proxy run-proxy`).
- `src/server.rs`: the strict server (`server run-server`) and its scenario.
- `src/web.rs`: the GraphQL API (async-graphql on axum) and the Host and Origin check.
  `schema.graphql` is its committed SDL; a test fails when it is stale
  (`cargo run -- client print-graphql-schema > schema.graphql`).
- `ui/index.html`: the single-page UI, embedded into the binary. Plain JavaScript; it speaks
  GraphQL over `fetch` and `graphql-transport-ws`.
- `src/position.rs`: resolving a `character` in a line for the UTF-8, UTF-16 and UTF-32 position
  encodings. The resolution and its theorems are Lean in a `lean!` block; `build.rs` checks the
  proofs with `lake build` and generates Rust into `.lean-rust/cache/`.
- `spec/metaModel.json`: the machine-readable specification (source commit in `spec/README.md`).
- `docs/rules.md`: every finding code with its rule; `tests/rules.rs` keeps it in step with `src/`.
- `docs/editors.md` and `tests/editors/`: the editor configurations and the batch scripts behind
  `make check-editors`.
- `tests/strict_pair.rs`: the strict client against the strict server; no message may have an
  error.

## Conventions

- Finding codes are `<area>/<kebab-case>` (`framing/missing-content-length`) and stay stable
  once released.
- A new finding code goes into `docs/rules.md` with its rule in the same commit.
- A behavior the specification forbids is an `error`. A behavior the specification allows but
  that implementations read differently is a `warning`.
- Types, methods, directions and capability bindings come from `spec/metaModel.json`.
- Commit `.lean-rust/cache/` and `lean/` with every change to a `lean!` block, so the crate builds
  without Lean (`nix build`, CI). Rules that should be proved correct are written in `lean!`.
- Commit messages, comments and docs are in English.

## Verification

```console
$ nix develop --command make check
$ nix build
```

After changing a check, run `make check-editors` and update the results in `docs/editors.md`.

UI changes are checked in a browser: start `client start-web-ui`, start `rust-analyzer`, send
`initialize`, `initialized` and a `didOpen`, reload the page, stop the server, and confirm the
browser console has no messages.
