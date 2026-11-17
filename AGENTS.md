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
- `src/document.rs`: lines and positions in a document's text, and the replacement of a range of
  characters, written and proved in a `lean!` block.
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
- `docs/editors.md` and `tests/editors/`: the editor configurations and the scripts behind
  `make check-editors` (eglot, lsp-mode, Neovim, Helix through tmux keystrokes, VS Code through
  VSCodium, `xvfb-run` and `@vscode/test-electron`). The editors live in the `editors` dev shell
  (`nix develop .#editors`); `tests/editors/vscode/node_modules` comes from `npm ci`.
- `tests/strict_pair.rs`: the strict client against the strict server; no message may have an
  error.
- `src/check.rs`: scoring (`check check-client`, `check check-server`, `check check-record`). A
  record is replayed session by session through a fresh monitor that is then ended by the side its
  `SessionEnd` line names (the client when the line is missing), so a session whose server was
  killed still gets the findings of its end. Each finding has the side it is about (`Party`), and a
  report counts the side under test only. `server run-server` reads `LSP_TOUCHSTONE_RECORD_FILE` in
  place of `--record-file` and `LSP_TOUCHSTONE_PROBE_UNDECLARED_REQUESTS` in place of
  `--probe-undeclared-requests`, and uses its process id as the session id. `tests/check.rs` runs
  the commands end to end.
- `action.yml`: the composite GitHub Action around `check`; `.github/workflows/action.yml` runs it
  through `uses: $/` on the strict server and on opencode.
- `docs/agents.md` and `tests/agents/`: the opencode and Claude Code configurations that start the
  strict server for `.tch` files, and what each agent did.

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

## Lean and Rust

Lean defines position conversion, document range replacement and progress transitions in
the `lean!` blocks. Rust converts their arguments and owns protocol I/O, document storage
and reporting. The generated functions called from Rust are `proved.resolve` and
`proved.offset` in `src/position.rs`, `proved.replace_range` (`replaceRange` in Lean) in
`src/document.rs`, and `proved.step` in `src/progress.rs`.

The position theorems are:

- `unitWidth_pos`: every scalar consumes at least one code unit.
- `unitWidth_spec`: scalar widths agree with UTF-8, UTF-16 and UTF-32.
- `resolve_offset`: converting a valid list index to code units and back preserves the index.
- `resolve_beyond`: offsets beyond the list's encoded length clamp to its end.
- `resolve_splits`: a reported split lies strictly inside the indexed scalar.
- `resolve_inside`: every offset strictly inside a scalar reports that split.
- `resolve_offset_roundtrip`: the String API preserves a valid index on an offset/resolve round trip.
- `resolve_clamps_past_end`: the String API clamps offsets beyond the line to its length.
- `resolve_inside_character`: the String API reports every offset strictly inside a scalar.
- `resolve_split_bounds`: every split reported by the String API names a scalar containing the offset.

The document theorems are:

- `replace_keeps_prefix`: list replacement preserves the characters before the range.
- `replace_keeps_suffix`: list replacement preserves the characters after the range.
- `replace_inserts`: list replacement puts the entire insertion at the range's start.
- `replace_length`: the result's length subtracts the removed range and adds the insertion.
- `replace_with_same_is_identity`: inserting the range's own contents preserves the list.
- `replace_whole`: replacing the entire list yields the insertion.
- `replaceRange_keeps_prefix`: String replacement preserves the prefix before the range.
- `replaceRange_keeps_suffix`: String replacement preserves the suffix after the range.
- `replaceRange_inserts`: String replacement contains the entire insertion at the requested start.
- `replaceRange_length`: String replacement has the expected scalar count for a valid range.
- `replaceRange_with_same_is_identity`: replacing a String range with its own contents preserves it.
- `replaceRange_whole`: replacing the entire String yields the insertion.

The progress theorems are:

- `ended_accepts_nothing`: an ended token rejects every progress event.
- `only_begin_leaves_created`: a successful first event begins the token.
- `only_report_or_end_leave_begun`: a begun token accepts reports or completion with their required states.
- `begun_never_returns_to_created`: an event cannot reset a begun token to created.
- `nothing_after_end`: every nonempty sequence after completion is rejected.
- `accepted_runs_begin_first`: every accepted nonempty sequence starts with begin.
- `begin_reports_end`: begin, any number of reports and completion reach the ended state.

`audit/` is a separate Lake package requiring `../lean` and
[lean-assay](https://github.com/conao3/lean-assay) at
`ef054d7c45ab4f9e14a201ae7e3bfbe9dd080abb`. Its three modules import each generated
`proved` namespace separately. The audit covers all four generated functions called from
Rust; every additional generated entry point needs useful theorems and an `#assay` line.
`lake -d audit build --wfail` rejects missing theorems, holes, vacuous hypotheses and functions
whose theorems reject zero mutants. Equivalence describes the inputs tried by the audit.
The position audit builds strings from Assay's Unicode character generator, covering every
UTF-8 and UTF-16 width, and derives `Inhabited Resolution` locally to enable its default-value
mutant. These instances apply to the audit module.

| Function | Mutants | Killed | Holes | Untested | Equivalent | Timed out | Stillborn |
|---|---:|---:|---:|---:|---:|---:|---:|
| `step` | 19 | 19 | 0 | 0 | 0 | 0 | 0 |
| `resolve` | 1 | 1 | 0 | 0 | 0 | 0 | 0 |
| `offset` | 3 | 3 | 0 | 0 | 0 | 0 | 0 |
| `replaceRange` | 3 | 3 | 0 | 0 | 0 | 0 | 0 |

All four entry points participate in the audit. Their helper functions have the list-level
and transition theorems listed above; the String theorems lift those properties to the API.

## Verification

```console
$ nix develop --command make check
$ nix build
```

`make check` runs treefmt, clippy (including regeneration), `lake -d lean build --wfail`,
`lake -d audit build --wfail`, tracked and untracked generated-file consistency checks, and
all Rust tests. Commit `lean/`, `.lean-rust/cache/` and the audit manifest with source changes.

After changing a check, run `make check-editors` and update the results in `docs/editors.md`.

UI changes are checked in a browser: start `client start-web-ui`, start `rust-analyzer`, send
`initialize`, `initialized` and a `didOpen`, reload the page, stop the server, and confirm the
browser console has no messages.
