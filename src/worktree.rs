//! Git worktrees from the sidebar: new (W), open (O), remove (D).
//!
//! The sidebar opens the tmux prompt, menu or confirmation. tmux then runs
//! this binary (`worktree-new`, `worktree-open`, `worktree-remove`), so the
//! git work never blocks the sidebar.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::t;
use crate::model::collect_agents;
use crate::panes::{ensure, jump};
use crate::tmux::gopt;

/// Option that the branch prompt writes the typed branch name into.
pub const INPUT_OPT: &str = "@agent_sidebar_input";

/// `git -C dir args`: (success, stdout, stderr), no timeout (a checkout can take time).
fn git(dir: &str, args: &[&str]) -> (bool, String, String) {
    match Command::new("git").arg("-C").arg(dir).args(args).stdin(Stdio::null()).output() {
        Ok(out) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ),
        Err(e) => (false, String::new(), e.to_string()),
    }
}

/// Quote a value for a tmux command string: double quotes, and `#` doubled
/// because `run-shell` expands formats.
pub fn quote(s: &str) -> String {
    let mut q = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' | '$' => {
                q.push('\\');
                q.push(c);
            }
            '#' => q.push_str("##"),
            _ => q.push(c),
        }
    }
    q.push('"');
    q
}

/// Quote a value for `sh` inside a tmux string: single quotes.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shell command that runs this binary with arguments, for `run-shell`.
pub fn self_cmd(args: &[&str]) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "agent-sidebar".into());
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe).to_string();
    std::iter::once(exe.as_str())
        .chain(args.iter().copied())
        .map(sh_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn message(client: &str, text: &str) {
    tmux!["display-message", "-c", client, text];
}

/// The main worktree of the repo that `dir` is in.
pub fn main_worktree(dir: &str) -> Option<String> {
    let (ok, common, _) = git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    if !ok || common.is_empty() {
        return None;
    }
    let common = Path::new(&common);
    // <main>/.git -> <main>
    Some(common.parent()?.to_string_lossy().into_owned())
}

/// (path, branch) of each worktree of the repo.
pub fn list(dir: &str) -> Vec<(String, String)> {
    let (ok, out, _) = git(dir, &["worktree", "list", "--porcelain"]);
    if !ok {
        return Vec::new();
    }
    let mut items = Vec::new();
    let mut path = None;
    let mut branch = String::new();
    for line in out.lines().chain(std::iter::once("")) {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = b.trim_start_matches("refs/heads/").to_string();
        } else if line == "detached" {
            branch = "(detached)".into();
        } else if line.is_empty()
            && let Some(p) = path.take() {
                items.push((p, std::mem::take(&mut branch)));
            }
    }
    items
}

/// Add a worktree for `branch` to the repo of `main` and return its path.
/// An existing local branch is checked out, otherwise it is created from HEAD.
/// A branch that already has a worktree returns that worktree.
pub fn add(main: &str, branch: &str) -> Result<String, String> {
    if let Some((path, _)) = list(main).into_iter().find(|(_, b)| b == branch) {
        return Ok(path);
    }
    let repo = Path::new(main).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let slug = branch.replace('/', "-");
    let base = match gopt("@agent_sidebar_worktree_dir") {
        dir if dir.is_empty() => {
            let parent = Path::new(main).parent().map(|p| p.to_string_lossy().into_owned());
            format!("{}/{repo}-worktrees", parent.unwrap_or_default())
        }
        dir => format!("{}/{repo}", dir.replacen('~', &std::env::var("HOME").unwrap_or_default(), 1)),
    };
    let path = format!("{base}/{slug}");
    let exists = git(main, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")]).0;
    let (ok, _, err) = if exists {
        git(main, &["worktree", "add", &path, branch])
    } else {
        git(main, &["worktree", "add", "-b", branch, &path])
    };
    if ok {
        Ok(path)
    } else {
        Err(format!("worktree: {}", err.lines().last().unwrap_or("git failed")))
    }
}

/// Type the command that starts claude into a new pane's shell:
/// `claude -n <name> "<first prompt>"`. The prompt goes through a file, so
/// quotes and newlines in it are safe.
pub fn start_claude(pane: &str, name: Option<&str>, prompt: Option<&str>) {
    let mut cmd = String::from("claude");
    if let Some(name) = name {
        cmd.push_str(&format!(" -n {}", sh_quote(name)));
    }
    if let Some(prompt) = prompt.filter(|p| !p.trim().is_empty()) {
        let dir = crate::resume::state_file().with_file_name("prompts");
        let file = dir.join(format!("{}.txt", pane.trim_start_matches('%')));
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&file, prompt).is_ok() {
            cmd.push_str(&format!(" \"$(cat {})\"", sh_quote(&file.to_string_lossy())));
        }
    }
    tmux!["send-keys", "-t", pane, "-l", cmd];
    tmux!["send-keys", "-t", pane, "Enter"];
}

/// Window after which new agent windows go: the window of `pane`.
fn window_of(pane: &str) -> String {
    tmux!["display-message", "-p", "-t", pane, "#{window_id}"].trim().to_string()
}

/// Open a new window after `after` in `dir`, start claude in it and show it.
/// The cursor goes to the sidebar of the new window.
fn open_agent_window(client: &str, after: &str, dir: &str) {
    let out = tmux!["new-window", "-a", "-t", after, "-c", dir, "-P", "-F", "#{pane_id} #{window_id}"];
    let mut it = out.split_whitespace();
    let (Some(pane), Some(window)) = (it.next(), it.next()) else {
        message(client, "worktree: tmux could not open a window");
        return;
    };
    start_claude(pane, None, None);
    tmux!["set-option", "-g", "@agent_sidebar_sel", pane];
    ensure(window);
    tmux!["select-window", "-t", window];
    tmux!["switch-client", "-c", client, "-t", window];
    // The after-new-window hook adds a sidebar at the same time, and the
    // younger of two sidebars quits. Wait (max 300 ms) for the one that stays.
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let sidebars: Vec<String> = tmux!["list-panes", "-t", window, "-F",
                                          "#{pane_id} #{@agent_sidebar}"]
            .lines()
            .filter_map(|l| l.strip_suffix(" 1").map(str::to_string))
            .collect();
        if sidebars.len() == 1 {
            tmux!["select-pane", "-t", sidebars[0]];
            break;
        }
    }
}

/// `worktree-new <client> <pane>`: the branch is in `@agent_sidebar_input`.
/// An existing local branch is checked out, otherwise it is created from HEAD.
pub fn cmd_new(client: &str, pane: &str) {
    let branch = gopt(INPUT_OPT).trim().to_string();
    tmux!["set-option", "-gu", INPUT_OPT];
    if branch.is_empty() {
        return;
    }
    let cwd = tmux!["display-message", "-p", "-t", pane, "#{pane_current_path}"].trim().to_string();
    let Some(main) = main_worktree(&cwd) else {
        message(client, t().wt_no_repo);
        return;
    };
    match add(&main, &branch) {
        Ok(path) => open_agent_window(client, &window_of(pane), &path),
        Err(err) => message(client, &err),
    }
}

/// `agent-new <client> <pane>`: a new agent in the repo (or directory) of
/// the agent in `pane`, in a new window after it. No worktree.
pub fn cmd_agent_new(client: &str, pane: &str) {
    let cwd = tmux!["display-message", "-p", "-t", pane, "#{pane_current_path}"].trim().to_string();
    let dir = crate::git::git_info(&cwd).map(|g| g.top).unwrap_or(cwd);
    open_agent_window(client, &window_of(pane), &dir);
}

/// `worktree-open <client> <pane> <path>`: show the agent in the worktree,
/// or start one in a new window.
pub fn cmd_open(client: &str, pane: &str, path: &str) {
    let inside = |cwd: &str| cwd == path || cwd.starts_with(&format!("{path}/"));
    let mut agents = collect_agents(None, &[]);
    if let Some(agent) = agents.iter_mut().find(|a| inside(&a.cwd)) {
        tmux!["switch-client", "-c", client, "-t", agent.pane];
        jump(agent, false);
        return;
    }
    open_agent_window(client, &window_of(pane), path);
}

/// `worktree-remove <client> <pane> <path> [--force]`: `git worktree remove`.
/// The branch stays. If git refuses because of changes, ask before `--force`.
/// The agent pane in the worktree closes, because its directory is gone.
pub fn cmd_remove(client: &str, pane: &str, path: &str, force: bool) {
    let Some(main) = main_worktree(path) else {
        message(client, t().wt_no_repo);
        return;
    };
    // the pane's directory, read before git deletes it
    let cwd = tmux!["display-message", "-p", "-t", pane, "#{pane_current_path}"].trim().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path);
    let (ok, _, err) = git(&main, &args);
    if !ok {
        if !force && (err.contains("modified or untracked") || err.contains("--force")) {
            let cmd = format!("run-shell -b {}",
                              quote(&self_cmd(&["worktree-remove", client, pane, path, "--force"])));
            crate::tmux::tmux_bg(&["confirm-before", "-t", client, "-p", t().wt_force, &cmd]);
        } else {
            message(client, &format!("worktree: {}", err.lines().last().unwrap_or("git failed")));
        }
        return;
    }
    if cwd == path || cwd.starts_with(&format!("{path}/")) {
        tmux!["kill-pane", "-t", pane];
    }
    message(client, &format!("{} {path}", t().wt_removed));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("a b"), "\"a b\"");
        assert_eq!(quote("x\"#$"), "\"x\\\"##\\$\"");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }
}
