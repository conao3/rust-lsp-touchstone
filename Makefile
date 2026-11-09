.PHONY: check
check:
	nix fmt -- --ci
	cargo clippy --all-targets -- -D warnings
	cargo test

# Runs eglot, lsp-mode, Neovim, Helix and VS Code against the strict server and against
# rust-analyzer behind the proxy and prints the findings of each session (tests/editors/run.sh).
.PHONY: check-editors
check-editors:
	nix develop .#editors --command bash -c 'cargo build && tests/editors/run.sh'

.PHONY: fmt
fmt:
	nix fmt

.PHONY: build
build:
	cargo build --release
