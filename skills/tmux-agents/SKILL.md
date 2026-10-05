---
name: tmux-agents
description: Start and drive other Claude Code agents in tmux windows, each in its own git worktree and branch. Use when the user asks to start, spawn or launch agents or Claude Code sessions (for example "start 5 agents named a, b, c in new worktrees with branches x, y, z"), to give a running agent a task, to wait for agents to finish, or to read what they did.
---

# tmux agents

`agent-sidebar` (in `~/.local/bin`) starts Claude Code agents in new tmux windows and talks to running ones. The agents show in the tmux agent sidebar. You run inside tmux, so the commands work from your Bash tool.

## Start agents

```bash
agent-sidebar spawn --name api --branch feat/api --prompt "Add the /health endpoint. Run the tests."
```

- `--name` (required): the agent's name. It is the tmux window name, the name in the sidebar and the Claude Code session name. It must be unique.
- `--branch`: the git branch. Default: the name. An existing local branch is checked out, otherwise it is created from the current `HEAD`. The worktree goes next to the repo: `<repo>/../<repo>-worktrees/<branch>` (`/` becomes `-`).
- `--no-worktree`: no worktree, the agent works in the repo itself. Do not use it for several agents in one repo: they would edit the same files.
- `--repo DIR`: the repo. Default: the current directory.
- `--session S`: the tmux session. Default: your own session.
- `--prompt TEXT` or `--prompt-file FILE`: the first task. Quotes and newlines are safe. Without it the agent waits for a prompt.

The window opens in the background: the user's view does not change. The command prints one JSON line: `{"name","pane","window","session","path","branch"}`.

The new agent is linked to you: the sidebar shows it under your agent (a tree the user can fold), and `list` shows you as its parent.

Several agents: run `spawn` once for each, one after the other. Give every agent a complete, self-contained task: it does not see this conversation.

## Drive agents

```bash
agent-sidebar list                      # name, status, branch, pane, path, parent (tab-separated)
agent-sidebar list --json
agent-sidebar prompt api "Also add a test for the 503 case."
agent-sidebar wait api --timeout 600    # until it finished (idle/done) or needs approval (waiting)
agent-sidebar read api --lines 60       # the last lines of its screen
```

- AGENT is the name or the tmux pane id (`%12`).
- Statuses: `working`, `waiting` (needs approval or an answer from the user), `done` (finished, not seen yet), `idle`, `unknown`.
- `wait` prints the status it stopped at. Exit code 1: timeout, or the agent is gone. Right after a `prompt`, `wait` first waits up to 10 s for the agent to start working. `--until` takes a comma list (default `idle,done,waiting`).
- A `waiting` agent needs the user: tell the user which agent waits. Do not approve its permission prompts yourself.
- Your Bash tool has a timeout. For long work, wait with `--timeout` in steps, or check with `list`.

## Get notified when an agent stops

To keep working while an agent works, run `wait` as a background command (Bash tool with `run_in_background: true`):

```bash
agent-sidebar wait api --timeout 3600
```

When the agent finishes (`idle`, `done`) or needs approval (`waiting`), the command exits and you get a notification. Then run `agent-sidebar read api` and decide what to do next. For several agents, start one background `wait` for each agent. A `waiting` agent needs the user: tell the user, as above.

## Example

The user: "start 3 agents in this repo: auth on branch feat/auth, billing on feat/billing, docs on docs/update, each fixes its part of issue 42".

```bash
agent-sidebar spawn --name auth --branch feat/auth --prompt "Issue 42: ... (the auth part). Commit on this branch when the tests pass."
agent-sidebar spawn --name billing --branch feat/billing --prompt "Issue 42: ... (the billing part). ..."
agent-sidebar spawn --name docs --branch docs/update --prompt "Issue 42: update the docs for ..."
agent-sidebar list
```

Then report the names, branches and worktree paths to the user.

## Notes

- Start agents only when the user asks for it. Each agent uses the user's Claude Code plan.
- Remove a worktree in the sidebar (`D` on the agent), or with `git worktree remove <path>`. The branch stays.
