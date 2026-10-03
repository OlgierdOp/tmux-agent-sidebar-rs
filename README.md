# tmux agent sidebar (Rust)

A Rust port of [tmux-agent-sidebar](https://github.com/OlgierdOp/tmux-agent-sidebar) (Python), 1:1 in behavior: same keys, statuses, tmux options and hook. See the Python repo's README for the features, keys and "How it works".

## Install and start

```bash
./install.sh
```

The script:

1. builds `target/release/agent-sidebar` (`cargo build --release`),
2. registers `hooks/claude-hook.sh` in `~/.claude/settings.json` and removes the Python version's hook,
3. loads `agent-sidebar.tmux` from `~/.tmux.conf`, in place of the Python version's line,
4. in a running tmux: turns off the running sidebars, reloads the config and turns the sidebar on.

It makes backups (`*.bak.<timestamp>`) and you can run it more than once.

Requirements: Linux, tmux 3.2 or later, Rust (cargo), `jq`.

## Back to the Python version

Run `./install.sh` in the Python repo, then press `prefix a` twice.

## Files

| File | Purpose |
|------|---------|
| `src/main.rs` | `toggle`, `ensure`, `next` commands and the TUI entry |
| `src/ui.rs` | Sidebar TUI: keys, worker thread, follow, J/K, colors |
| `src/screen.rs` | Cell buffer that writes only changed cells (like curses) |
| `src/model.rs` | Agents from tmux panes, sorting |
| `src/live.rs` | Claude Code session files (`~/.claude/sessions`), background sessions |
| `src/transcript.rs` | Context tokens and turn end from the transcript |
| `src/git.rs` | Branch and worktree |
| `src/procs.rs` | Process tree (`/proc`) |
| `src/panes.rs` | Sidebar panes, `jump` |
| `src/tmux.rs` | tmux calls |
| `src/config.rs` | Settings, UI strings, statuses |
| `hooks/claude-hook.sh` | Claude Code hook (same as the Python version) |
| `agent-sidebar.tmux` | tmux key bindings and hooks |
| `install.sh` | Build, install and start |
