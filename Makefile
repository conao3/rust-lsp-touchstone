.PHONY: check
check:
	nix fmt -- --ci
	cargo clippy --all-targets -- -D warnings
	cargo test

# Runs eglot and Neovim against the strict server and against rust-analyzer behind the proxy
# and prints the findings of each session (tests/editors/run.sh).
.PHONY: check-editors
check-editors:
	cargo build
	tests/editors/run.sh

.PHONY: fmt
fmt:
	nix fmt

.PHONY: build
build:
	cargo build --release
