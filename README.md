# tmux agent sidebar

A sidebar for tmux that lists every [Claude Code](https://claude.com/claude-code) agent running in your tmux panes. For each agent it shows:

- the status (working, waiting for you, finished, idle),
- the git branch, commits ahead/behind and changed files,
- the worktree (linked git worktrees are marked),
- the context size in tokens.

You can move between agents with `j`/`k` and show an agent's window with `Enter`. The cursor stays in the sidebar while you do this. You can search and filter the list, add and remove git worktrees, and the agents come back after a tmux restart.

This is the Rust version. It started as a 1:1 port of the [Python version](https://github.com/OlgierdOp/tmux-agent-sidebar), which is no longer developed.

```
 AGENTS ●1 ●1 ◐1
─────────────────────────────────────────
━━ agents ●1 ◐1 ━━━━━━━━━━━━━━━━━━━━━━━━━
 ◓ 1 auth                           142k
   ⎇ main ↑2 ●3
   ⌂ ~/repos/app
 ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄
 ● 2 login                           38k
   ⎇ feat/login
   ⊕ ~/repos/app-worktrees/feat-login [wt]
 ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄
━━ other ●1 ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 ● 3 api                            211k
   ⎇ fix/timeout ↓1
   ⌂ ~/repos/api
```

## Requirements

- Linux (the agent detection reads `/proc`)
- tmux 3.2 or later (tested with 3.4)
- Rust and cargo (to build)
- `jq` (used by the Claude Code hook)
- Claude Code
- Optional: tmux-resurrect, to resume the agents after a tmux restart

A 256-color terminal is recommended.

## Installation

```bash
git clone https://github.com/OlgierdOp/tmux-agent-sidebar-rs.git
cd tmux-agent-sidebar-rs
./install.sh
```

`install.sh`:

1. builds `target/release/agent-sidebar` (`cargo build --release`),
2. adds `hooks/claude-hook.sh` to the hooks in `~/.claude/settings.json` and removes the hook of the Python version,
3. adds `source-file .../agent-sidebar.tmux` to `~/.tmux.conf`, in place of the Python version's line, or before the TPM init line,
4. in a running tmux: turns off the running sidebars, reloads the config and turns the sidebar on.

Before it changes a file, the script makes a backup (`*.bak.<timestamp>`). You can run it more than once. An error elsewhere in `~/.tmux.conf` does not stop it. Running Claude Code sessions pick up the new hooks on their next event.

### Manual installation

1. `cargo build --release`
2. Add this line to `~/.tmux.conf`:

   ```tmux
   source-file /path/to/tmux-agent-sidebar-rs/agent-sidebar.tmux
   ```

3. Register `hooks/claude-hook.sh` as a `command` hook in `~/.claude/settings.json` for these events: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `PermissionRequest`, `Stop`, `SessionEnd`.

## Usage

| Key          | Action |
|--------------|--------|
| `prefix a`   | Show/hide the sidebar in all windows |
| `prefix Tab` | Go into the agent that waits for you (red first, then green, current session first) |

Keys in the sidebar:

| Key             | Action |
|-----------------|--------|
| `j` / `k`, arrows | Move the selection |
| `g` / `G`       | First / last agent |
| `J` / `K`       | Move the selected agent down / up (swaps the tmux windows). At the edge of a session: move the whole session. |
| `Enter` / `o`   | Show the agent's window. The cursor stays in the sidebar. |
| `i`             | Go into the agent's pane |
| `1`–`9`         | Show the agent with this number |
| `Tab`           | Select the next agent that waits for you |
| `/`             | Search (name, window, session, branch, path). `Enter` keeps the filter, `Esc` clears it. |
| `w` / `d` / `b` | Show only waiting / done / busy agents (press again to turn off) |
| `a` / `Esc`     | Show all agents (clears the search and the state filter) |
| `s`             | Show only the home session / all sessions |
| `W`             | New git worktree: asks for a branch, starts claude in it in a new window |
| `O`             | Menu of the repo's worktrees: show its agent, or start one |
| `D`             | Remove the selected agent's linked worktree (asks first) |
| `n`             | Give the agent a name |
| `c`             | Next selection background color (live preview) |
| `r`             | Refresh git data |
| mouse click     | Show the agent's window |

When you show an agent with `Enter`, the agent becomes the last active pane in its window. Your normal pane navigation (for example `Ctrl+l` with vim-tmux-navigator, or `prefix ;`) goes into it.

The selection follows you. If you move to another agent with tmux keys or a script (`select-window`, `select-pane`), the sidebar selects that agent.

### Search and filters

The search and the state filters are local to one sidebar pane, so two tmux clients can use different filters. The active filters show in the title line, for example `[waiting /api]`. The counts in the title line always cover all agents. The numbers `1`–`9`, `j`/`k` and `Tab` work on the filtered list.

### Sessions

Agents are grouped by tmux session. The home session is the session where you turned the sidebar on, and it is at the top by default.

To change the order, select an agent and press `J` (down) or `K` (up):

- Inside a session, the agent swaps places with the next or previous agent. The sidebar swaps the tmux windows (`swap-window`), so the window numbers in the tmux status bar change too. You stay in the window you look at. Two agents in one window swap panes.
- On the last agent of a session, `J` moves the whole session down. On the first agent, `K` moves it up. All sidebars use the same session order. It is stored in `@agent_sidebar_order` until tmux restarts. Sessions that are not in the order yet (new sessions) go after the others.

The agent numbers (`1`–`9`) always follow the order in the list.

### Worktrees

- `W` asks for a branch name. An existing local branch is checked out, otherwise the branch is created from `HEAD`. The worktree goes to `<repo>/../<repo>-worktrees/<branch>` (`/` in the branch becomes `-`), or to `<dir>/<repo>/<branch>` when `@agent_sidebar_worktree_dir` is set. A new tmux window opens after the agent's window, `claude` starts in it, and the cursor goes to that window's sidebar.
- `O` lists the worktrees of the selected agent's repo. A worktree that has an agent shows that agent. A worktree without one gets a new window with `claude`.
- `D` works on an agent in a linked worktree (`⊕ ... [wt]`). It asks, runs `git worktree remove`, and closes the agent's pane, because its directory is gone. If git refuses because of changes, it asks again before `--force`. The branch is never deleted.

### Resume after a tmux restart

With tmux-resurrect, a restore brings the panes back as plain shells. `agent-sidebar.tmux` sets `@resurrect-hook-post-restore-all`, so after a restore the sidebar:

1. types `claude --resume <session id>` in each restored shell that ran an agent, when the shell is in the same directory and the session's transcript still exists,
2. closes the restored panes that were sidebars (plain shells with the sidebar's width at the left edge),
3. turns the sidebars on.

The data comes from `~/.local/state/tmux-agent-sidebar/agents.json` (`$XDG_STATE_HOME`), which the visible sidebar keeps up to date. Turn it off with `set -g @agent_sidebar_resume off`. If you set `@resurrect-hook-post-restore-all` yourself, load `agent-sidebar.tmux` before your line and call `agent-sidebar resume` from your hook.

### Statuses

| Symbol      | Meaning |
|-------------|---------|
| `●` red     | Waiting for you to accept: a permission prompt or a question |
| `●` green   | Finished, and you did not look at it yet. It turns idle when you show the agent. |
| `◐` yellow  | Working (it spins) |
| `○` gray    | Idle. Also after you stop the agent (`Esc`, `Ctrl+C`) or reject a permission prompt. |
| `?`         | No hook data yet (for example, a session started before the installation) |

Next to the branch: `↑n` commits ahead of the upstream, `↓n` behind, `●n` changed files (with untracked files).

## Configuration

| Setting | Where | Default |
|---------|-------|---------|
| Selection background | `set -g @agent_sidebar_bg N` (xterm-256 color number) | `23` |
| Sidebar width | `AGENT_SIDEBAR_WIDTH` environment variable | `42` |
| Worktree directory | `set -g @agent_sidebar_worktree_dir DIR` | `<repo>/../<repo>-worktrees` |
| Resume after restore | `set -g @agent_sidebar_resume off` | on |
| UI language | `LANG` and `STRINGS` in `src/config.rs` (only `en` for now) | `en` |
| Key bindings | `agent-sidebar.tmux` | `prefix a`, `prefix Tab` |

To choose a background color, press `c` in the sidebar until you like the color. The number shows in the top-right corner for 3 seconds. Write it into your tmux config to keep it after a tmux restart.

## How it works

- **One sidebar pane per window.** A switch between windows does not move or resize panes, so nothing flickers. The panes share their state through global tmux options:
  - `@agent_sidebar_on`
  - `@agent_sidebar_sel`
  - `@agent_sidebar_only`
  - `@agent_sidebar_home`
  - `@agent_sidebar_bg`
  - `@agent_sidebar_order`
  - `@agent_sidebar_gen` (changes when `J`/`K` swaps windows, so the other sidebars collect the data again)
- **Fast selection sync.** A selection change sends `F12` to the other sidebar panes, so they redraw at once. `Enter` waits (max ~150 ms) until the target window's sidebar has drawn the new selection, then it switches the window.
- **Window, session and pane switches.** The tmux hooks `session-window-changed`, `client-session-changed`, `after-select-pane` and `after-new-window` do two things:
  - they add a sidebar to a window on the first visit,
  - they send `F12` to the sidebar of the window you switch to, so it redraws at once (a few ms).

  tmux evaluates the conditions itself, and `run-shell -C` runs a tmux command, so a normal switch starts no process.
- **Reliable selection follow.** The visible sidebar compares the current window and active pane with the last pair it handled (`@agent_sidebar_focus`). It compares states, not events, so fast switching cannot make it miss a change. Visibility comes from `window_active_clients`, because tmux updates `session_attached` lazily.
- **Non-blocking UI.** A worker thread collects the agent data (process tree, git, tokens). The main loop only handles keys, `F12` and drawing, so it never waits for the data. Prompts, menus and confirmations (`command-prompt`, `display-menu`, `confirm-before`) run in the background, because tmux returns from them only when you close them. The worktree work runs in `agent-sidebar worktree-*` commands that tmux starts.
- **Drawing.** `src/screen.rs` keeps a cell buffer and writes only the cells that changed, with the same color codes that curses writes for `tmux-256color`.
- **Agent detection.** The sidebar reads `tmux list-panes -a` and the process tree of each pane (`/proc/PID/task/TID/children`, with a full `/proc` scan as fallback). Any pane with a `claude` process in it is an agent.
- **Status.** Two sources:
  - Claude Code writes the state of each session (`busy`, `waiting`, `idle`) to `~/.claude/sessions/<pid>.json`. The sidebar finds the `claude` process of the pane and reads this file. When the pane runs only a client of a background session (`parkedJobId`), the sidebar reads the file of the background session (`jobId`). The file is correct at once, also after `Esc`, `Ctrl+C` or a rejected permission prompt, when no hook runs.
  - Every 0.1 s each sidebar checks these files (only a `stat` when nothing changed), so a status change shows in about 50 ms.
  - When a turn ends (`busy` → `idle`), the last message of the transcript tells how. An assistant reply that ended the turn means "finished": green, or idle when you look at the agent. Your prompt or `[Request interrupted by user]` at the end means you stopped it: idle.
  - `hooks/claude-hook.sh` runs on Claude Code hook events. It stores `@agent_status`, `@agent_ts` and `@agent_transcript` as tmux pane options. Its `Stop` event also sets "done". No hook runs for a background session.
  - Only a permission prompt or a question makes the dot red. A late notification after the turn ended does not.
  - Without a session file (older Claude Code), the sidebar uses the hook status. It sets idle when the transcript ends with `[Request interrupted by user]`, or when the `idle_prompt` notification comes (after about 60 s).
- **Tokens.** The context size comes from the `usage` of the last assistant message in the session transcript (input + cache + output tokens).
- **Git.** The sidebar runs `git rev-parse` in the agent's working directory, and `git status --porcelain=v2 --branch` once per repo (cached for 3 s). `[wt]` marks a linked worktree.
- **Name.** The name you give with `n`, else the window name. When tmux names the window automatically (after the command of the active pane), the sidebar uses the repo or directory name.
- **Refresh.** The visible sidebar refreshes its data every 1 s, hidden ones every 5 s. As a safety net, the visible sidebar also polls tmux state every 1 s (hidden: 2 s).

## Files

| File | Purpose |
|------|---------|
| `src/main.rs` | Commands (`toggle`, `ensure`, `next`, `resume`, `worktree-*`) and the TUI entry |
| `src/ui.rs` | Sidebar TUI: keys, worker thread, follow, filters, J/K, colors |
| `src/screen.rs` | Cell buffer that writes only changed cells (like curses) |
| `src/model.rs` | Agents from tmux panes, sorting |
| `src/live.rs` | Claude Code session files (`~/.claude/sessions`), background sessions |
| `src/transcript.rs` | Context tokens and turn end from the transcript |
| `src/git.rs` | Branch, worktree, ahead/behind and changes |
| `src/worktree.rs` | `W` / `O` / `D`: add, open and remove worktrees |
| `src/resume.rs` | State file and `resume` after a tmux-resurrect restore |
| `src/procs.rs` | Process tree (`/proc`) |
| `src/panes.rs` | Sidebar panes, `jump` |
| `src/tmux.rs` | tmux calls |
| `src/config.rs` | Settings, UI strings, statuses |
| `hooks/claude-hook.sh` | Claude Code hook that writes the agent status into tmux |
| `agent-sidebar.tmux` | tmux key bindings and hooks |
| `install.sh` | Build, install and start |
| `ROADMAP.md` | Ideas for later |

## Uninstall

1. Remove the `source-file .../agent-sidebar.tmux` line from `~/.tmux.conf`.
2. Remove the `claude-hook.sh` entries from `~/.claude/settings.json`.
3. Restart tmux, or unbind `prefix a` / `prefix Tab`, remove the `[42]` hooks and `@resurrect-hook-post-restore-all` by hand.

## License

MIT
