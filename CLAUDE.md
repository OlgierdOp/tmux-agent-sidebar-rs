# CLAUDE.md

## Project

tmux agent sidebar (Rust): a tmux sidebar that lists the Claude Code agents running in tmux panes. It shows each agent's status, branch, git status, worktree and token count, and manages worktrees and resume after a tmux restart. See `README.md` for features and architecture. The `Files` table in the README lists the modules.

## Rules

- **Keep `README.md` in sync with the code.** When you change behavior, keys, statuses, options, files, requirements or installation steps, update the matching README section in the same change. Before you finish a task, compare the README with the code:
  - the key tables (`Ui::handle_key` in `src/ui.rs` and `agent-sidebar.tmux`),
  - the status table (`status_def` in `src/config.rs`),
  - the configuration table (every `@agent_sidebar_*` option the code reads),
  - "How it works",
  - the "Files" table.
- Write all code, comments, UI strings, docs and commit messages in English.
- Put UI strings in `Strings` / `STRINGS` in `src/config.rs`. Do not hard-code user-visible text.
- Keep dependencies few (now: crossterm, serde_json, unicode-width). Ask before you add one.
- Keep a normal window switch free of process spawns. The tmux hooks must check their condition in tmux formats before they call `run-shell`.
- Keep the per-tick path cheap: the main loop runs every 0.1 s in every sidebar pane. Only `stat` calls and in-memory work there; process spawns only on a change or a key. Slow work (git, process scans) goes to the worker thread.
- Store shared sidebar state in global tmux options (`@agent_sidebar_*`), not in files. The only file is the resume state (`src/resume.rs`).
- Never call `command-prompt`, `display-menu` or `confirm-before` with `tmux!`: they return only when the prompt closes. Use `tmux::tmux_bg`.
- In tmux, `command-prompt -p` splits prompts at commas: no commas in prompt strings. `display-menu` takes the client with `-c`, `command-prompt` and `confirm-before` with `-t`.
- `cargo build --release` without warnings, `cargo clippy` clean, `cargo test` passing.

## Testing

Test on a separate tmux server. Do not touch the user's real tmux server or config. Keep each test command short (seconds, not minutes): trigger the refresh (`r`, `F12`) instead of waiting for periodic updates, and use timeouts that match the expected time.

```bash
unset TMUX TMUX_PANE
tmux -L agtest -f /dev/null new-session -d -s agents ...
tmux -L agtest -f /dev/null source-file ./agent-sidebar.tmux
# hooks need an attached client:
tmux -L agouter -f /dev/null new-session -d "env -u TMUX TERM=tmux-256color tmux -L agtest attach"
export TMUX="$(tmux -L agtest display -p '#{socket_path}'),1,0"
```

- Pass `-f /dev/null` on every tmux call: a call that starts a server must never read `~/.tmux.conf`. `kill-server` returns before the server is gone, so wait until `tmux -L agtest ls` fails before you start a new one.
- A copy of `sleep` named `claude` works as a fake agent process. Put a fake `claude` command first in `PATH` when a test makes the sidebar type `claude` (worktrees, resume), so the real Claude Code never starts.
- Point `CLAUDE_CONFIG_DIR` and `XDG_STATE_HOME` to test directories (`tmux set-environment -g ...`) to fake session files and the resume state.
- Create a new directory for each test run (`mktemp -d`). Do not use `rm -rf`.
- To fake hook events, pipe JSON into the hook with `TMUX_PANE` set:

  ```bash
  echo '{"hook_event_name":"Stop"}' | TMUX_PANE=%1 hooks/claude-hook.sh
  ```

- Check the rendered sidebar with `tmux capture-pane -p [-e]`. Each sidebar stores the agent it shows as the pane option `@agent_sidebar_drawn`.
- Send keys to a sidebar with `tmux send-keys -t <sidebar pane> <key>`. Type into a prompt with `tmux send-keys -K -c <client> <keys>`. `send-keys -K` does not reach `display-menu`: test a menu item's command through `source-file` instead.
- Read the client's real window and pane with `tmux list-clients -F '#{window_id} #{pane_id}'`. `display -p -c <client>` does not show the client's session.
- After a change to switching or selection, run a stress test: many random window, session and pane switches with 0–30 ms gaps, then check the visible sidebar.
- Kill the test servers when you finish.
