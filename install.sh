#!/usr/bin/env bash
# Builds the sidebar, installs the Claude Code hooks (~/.claude/settings.json) and the
# tmux bindings (~/.tmux.conf), then starts it in the running tmux.
# Replaces the Python version (tmux_agent_sidebar) if it is installed.
# Idempotent: safe to run more than once. Makes backups before it changes a file.
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
HOOK="$DIR/hooks/claude-hook.sh"
BIN="$DIR/target/release/agent-sidebar"
SETTINGS="$HOME/.claude/settings.json"
TMUX_CONF="$HOME/.tmux.conf"
SNIPPET="$DIR/agent-sidebar.tmux"
LINE="source-file $SNIPPET"
STAMP="$(date +%Y%m%d%H%M%S)"

for cmd in cargo tmux jq; do
  command -v "$cmd" >/dev/null || { echo "$cmd is required"; exit 1; }
done

# --- build
(cd "$DIR" && cargo build --release --quiet)
echo "✓ built $BIN"

# --- Claude Code hooks: this hook, without the hook of the Python version
mkdir -p "$(dirname "$SETTINGS")"
[ -f "$SETTINGS" ] || echo '{}' > "$SETTINGS"
cp "$SETTINGS" "$SETTINGS.bak.$STAMP"
tmp=$(mktemp)
jq --arg cmd "$HOOK" '
  def entry: {hooks: [{type: "command", command: $cmd, timeout: 5}]};
  def ours: (.command // "") as $c | $c == $cmd or ($c | endswith("/tmux_agent_sidebar/hooks/claude-hook.sh"));
  .hooks //= {}
  | reduce ("SessionStart","UserPromptSubmit","PreToolUse","PostToolUse",
            "Notification","PermissionRequest","Stop","SessionEnd") as $ev (.;
      .hooks[$ev] = ((.hooks[$ev] // [])
                     | map(select(all(.hooks[]?; ours | not)))
                     + [entry]))
' "$SETTINGS" > "$tmp" && mv "$tmp" "$SETTINGS"
echo "✓ hooks in $SETTINGS"

# --- the binary in PATH and the skill, so agents can start other agents
mkdir -p "$HOME/.local/bin" "$HOME/.claude/skills"
ln -sfn "$BIN" "$HOME/.local/bin/agent-sidebar"
ln -sfn "$DIR/skills/tmux-agents" "$HOME/.claude/skills/tmux-agents"
echo "✓ ~/.local/bin/agent-sidebar and the tmux-agents skill"

# --- ~/.tmux.conf: replace the Python version's line, or add ours
touch "$TMUX_CONF"
if ! grep -qxF "$LINE" "$TMUX_CONF"; then
  cp "$TMUX_CONF" "$TMUX_CONF.bak.$STAMP"
  tmp=$(mktemp)
  awk -v line="$LINE" '
    /^source-file .*\/agent-sidebar\.tmux$/ { if (!done) print line; done = 1; next }
    /^run .*tpm\/tpm/ && !done { print "# Agent sidebar"; print line; print ""; done = 1 }
    { print }
    END { if (!done) { print ""; print "# Agent sidebar"; print line } }
  ' "$TMUX_CONF" > "$tmp" && cat "$tmp" > "$TMUX_CONF" && rm -f "$tmp"
  echo "✓ $TMUX_CONF loads $SNIPPET"
fi

# --- start it in the running tmux
if tmux list-sessions >/dev/null 2>&1; then
  # an error elsewhere in the config must not stop the install: load our file in any case
  tmux source-file "$TMUX_CONF" || echo "! $TMUX_CONF has errors (see above)"
  tmux source-file "$SNIPPET"
  # restart the running sidebars (Python or Rust: they are all marked @agent_sidebar)
  # in the sessions that have the sidebar on
  "$BIN" reload
  if [ -n "${TMUX:-}" ] && [ "$(tmux display -p '#{@agent_sidebar_on}')" != 1 ]; then
    "$BIN" toggle
  fi
  if [ -n "${TMUX:-}" ]; then
    echo "✓ sidebar is on in this session. prefix + a hides it."
  else
    echo "✓ tmux reloaded. Press prefix + a in tmux to show the sidebar."
  fi
else
  echo "Done. Start tmux and press prefix + a to show the sidebar."
fi
