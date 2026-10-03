# Roadmap

Done: search and state filters, resume after a tmux restart, worktrees from the sidebar, git status. Notifications and sounds are left to Claude Code.

Ideas from [herdr](https://github.com/herdrdev/herdr):

- **Bounded process scans.** herdr limits the `/proc` work per pane (number of processes, task entries and bytes read). Our walk has a depth limit only. Add a process count limit.
- **Foreground process group.** herdr reads `tpgid` from `/proc/<pid>/stat` to find the job in the foreground of the terminal. This is more exact than "any `claude` process in the tree" (for example a `claude` started in the background of a shell).
- **Subagent hook events.** herdr ignores Claude Code hook events that have `agent_id` (subagents). Check if our hook must do the same for `Stop` and `Notification`.
- **State separate from drawing.** herdr keeps state as plain data and makes rendering a pure function of it, so the state logic is testable without a terminal. `Ui` mixes both; split it.
- **Multiplicative paths.** herdr reviews every change in per-tick and per-pane paths for its cost × panes × clients. Keep doing this for new features.
