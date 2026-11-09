# AGENTS.md

`lsp-touchstone` is a Rust CLI that checks Language Server Protocol implementations: a checking
proxy, a strict client and a strict server sharing one set of rules.

## Layout

- `src/main.rs`: the command definitions (clap derive, AWS CLI grammar), `--output` rendering
  and exit codes 252/254/255.
- `src/finding.rs`: a finding (severity, code, message, byte offset).
- `src/framing.rs`: the base protocol framing reader and its checks.
- `src/position.rs`: resolving a `character` in a line for the UTF-8, UTF-16 and UTF-32 position
  encodings. The resolution and its theorems are Lean in a `lean!` block; `build.rs` checks the
  proofs with `lake build` and generates Rust into `.lean-rust/cache/`.
- `spec/metaModel.json`: the machine-readable specification (source commit in `spec/README.md`).

## Conventions

- Finding codes are `<area>/<kebab-case>` (`framing/missing-content-length`) and stay stable
  once released.
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
