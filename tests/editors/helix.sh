#!/usr/bin/env bash
# Drives Helix against lsp-touchstone through keystrokes in a private tmux server: Helix has no
# batch mode. TOUCHSTONE_COMMAND is the language server command as a JSON array,
# TOUCHSTONE_FILE the Rust file to edit, and TOUCHSTONE_MODE `server' or `proxy'.
set -euo pipefail

file=$TOUCHSTONE_FILE
project=$(dirname "$(dirname "$file")")
config=$project/.config
socket=$project/tmux.sock
mkdir -p "$project/.helix" "$config/helix"

jq -r '"[language-server.lsp-touchstone]\ncommand = \(.[0] | tojson)\nargs = \(.[1:] | tojson)\n\n[[language]]\nname = \"rust\"\nlanguage-servers = [\"lsp-touchstone\"]"' \
  <<< "$TOUCHSTONE_COMMAND" > "$project/.helix/languages.toml"
printf '[editor]\nauto-completion = true\ncompletion-timeout = 5\n' > "$config/helix/config.toml"

keys() { tmux -S "$socket" send-keys -t helix "$@"; sleep 0.7; }

tmux -S "$socket" new-session -d -s helix -x 160 -y 40 -c "$project" \
  "env XDG_CONFIG_HOME='$config' hx --log '$project/helix.log' '$file'"
sleep 4
keys / main Enter
keys Space k
sleep 1
keys Escape
keys g d
sleep 1
keys g e o
tmux -S "$socket" send-keys -t helix -l "// typed in helix 😀"
sleep 1
keys Escape Escape
if [ "$TOUCHSTONE_MODE" = server ]; then
  keys g g Space a
  sleep 1
  keys Enter
  sleep 2
fi
keys : w Enter
sleep 1
tmux -S "$socket" capture-pane -p -t helix > "$project/screen.txt"
keys : q ! Enter
for _ in $(seq 1 50); do
  tmux -S "$socket" has-session -t helix 2>/dev/null || break
  sleep 0.2
done
tmux -S "$socket" kill-server 2>/dev/null || true
