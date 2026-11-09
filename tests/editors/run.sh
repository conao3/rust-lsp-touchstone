#!/usr/bin/env bash
# Runs eglot and Neovim against the strict server and against rust-analyzer behind the proxy,
# records every session, and prints the findings of each. The findings are about the editors and
# the server under test; this script fails only when a run itself fails.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
bin="$root/target/debug/lsp-touchstone"
out=${TOUCHSTONE_OUT:-$root/test-out/editors}
rm -rf "$out"
mkdir -p "$out"

make_project() {
  local dir=$1
  mkdir -p "$dir/src"
  printf '[package]\nname = "sample"\nversion = "0.1.0"\nedition = "2024"\n' > "$dir/Cargo.toml"
  printf 'fn main() {\n    let s = "日本😀";\n    println!("{s}");\n}\n' > "$dir/src/main.rs"
}

run() {
  local editor=$1 mode=$2
  local dir="$out/$editor-$mode"
  make_project "$dir"
  local record="$dir/record.ndjson"
  local command
  if [ "$mode" = server ]; then
    command=$(jq -cn --arg bin "$bin" --arg record "$record" '[$bin, "server", "run-server", "--record-file", $record]')
  else
    command=$(jq -cn --arg bin "$bin" --arg record "$record" '[$bin, "proxy", "run-proxy", "--record-file", $record, "--server-command-args", "rust-analyzer"]')
  fi
  local env=(TOUCHSTONE_COMMAND="$command" TOUCHSTONE_FILE="$dir/src/main.rs" TOUCHSTONE_MODE="$mode")
  case $editor in
    eglot) env "${env[@]}" emacs -Q --batch -l "$root/tests/editors/eglot.el" > "$dir/editor.log" 2>&1 ;;
    neovim) env "${env[@]}" nvim --headless --clean -u NONE -l "$root/tests/editors/nvim.lua" > "$dir/editor.log" 2>&1 ;;
  esac
  local messages errors warnings
  messages=$(wc -l < "$record")
  errors=$(jq -s '[.[].Findings[] | select(.Severity == "error")] | length' "$record")
  warnings=$(jq -s '[.[].Findings[] | select(.Severity == "warning")] | length' "$record")
  printf '\n## %s, %s: %s messages, %s errors, %s warnings\n' "$editor" "$mode" "$messages" "$errors" "$warnings"
  jq -rs '[.[] | . as $m | .Findings[] | {Severity, Code, Direction: $m.Direction, Method: ($m.Method // "")}]
    | group_by([.Severity, .Code, .Direction, .Method])
    | map("- \(.[0].Severity) \(.[0].Code) (\(.[0].Direction) \(.[0].Method)): \(length)") | .[]' "$record"
}

for editor in eglot neovim; do
  for mode in server proxy; do
    run "$editor" "$mode"
  done
done
