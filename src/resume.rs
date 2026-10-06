//! Resume the agents after a tmux restart (tmux-resurrect).
//!
//! The visible sidebar writes the running agents (pane position, Claude Code
//! session id, cwd) and the sidebar pane positions to a state file.
//! tmux-resurrect restores panes as plain shells; its post-restore hook runs
//! `agent-sidebar resume`, which starts `claude --resume <id>` in each agent
//! pane and closes the panes that were sidebars.
//!
//! The state file also keeps the names of the sessions with the sidebar on,
//! so a session gets its sidebar back after a tmux restart.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::config::{ORDER_SEP, SIDEBAR_WIDTH};
use crate::live::CLAUDE_DIR;
use crate::model::Agent;
use crate::panes::ensure;
use crate::tmux::{current_session, gopt, sidebar_on};
use crate::transcript::project_dir_name;

const SHELLS: [&str; 9] = ["bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "nu", "elvish"];

pub fn state_file() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| format!("{}/.local/state", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(base).join("tmux-agent-sidebar").join("agents.json")
}

/// The names of the sessions with the sidebar on, as `:a:b:` (tmux does not
/// allow ":" in session names). The `session-created` hook turns the sidebar
/// on in a new session with one of these names.
pub const SAVED_OPT: &str = "@agent_sidebar_saved";
/// Set by tmux-resurrect's pre-restore hook until `resume` runs: a restored
/// session must not get a sidebar while resurrect restores its panes.
pub const RESTORING_OPT: &str = "@agent_sidebar_restoring";

fn split_names(s: &str) -> Vec<String> {
    s.split(ORDER_SEP).filter(|n| !n.is_empty()).map(str::to_string).collect()
}

fn join_names(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    format!("{ORDER_SEP}{}{ORDER_SEP}", names.join(ORDER_SEP))
}

/// The saved names, updated with the live sessions (`name=on:` each): a live
/// session counts as it is now, a session that is gone keeps its saved state.
fn merge_sessions(saved: &str, live: &str) -> Vec<String> {
    let mut names = split_names(saved);
    for entry in live.split(ORDER_SEP) {
        let Some((name, on)) = entry.rsplit_once('=') else { continue };
        let known = names.iter().any(|n| n == name);
        if on == "1" && !known {
            names.push(name.to_string());
        } else if on != "1" && known {
            names.retain(|n| n != name);
        }
    }
    names
}

/// Update `@agent_sidebar_saved` from the live sessions. Returns the names.
pub fn sync_sessions() -> Vec<String> {
    let out = tmux!["display-message", "-p",
                    format!("#{{{SAVED_OPT}}}\t#{{S:#{{session_name}}=#{{@agent_sidebar_on}}{ORDER_SEP}}}")];
    let Some((saved, live)) = out.trim_end_matches('\n').split_once('\t') else {
        return split_names(&gopt(SAVED_OPT));
    };
    let names = merge_sessions(saved, live);
    let value = join_names(&names);
    if value != saved {
        tmux!["set-option", "-g", SAVED_OPT, value];
    }
    names
}

/// Before per-session sidebars, `@agent_sidebar_on` was global: move it to
/// every session.
pub fn migrate_global() {
    if gopt("@agent_sidebar_on") != "1" {
        return;
    }
    tmux!["set-option", "-gu", "@agent_sidebar_on"];
    for session in tmux!["list-sessions", "-F", "#{session_id}"].split_whitespace() {
        tmux!["set-option", "-t", session, "@agent_sidebar_on", "1"];
    }
}

/// `agent-sidebar load`: run when tmux loads `agent-sidebar.tmux`. Reads the
/// saved session names into `@agent_sidebar_saved` (after a tmux restart).
pub fn cmd_load() {
    migrate_global();
    if gopt(SAVED_OPT).is_empty()
        && let Ok(text) = std::fs::read_to_string(state_file())
        && let Ok(state) = serde_json::from_str::<Value>(&text)
    {
        let names: Vec<String> = state["sessions"].as_array().into_iter().flatten()
            .filter_map(Value::as_str).map(str::to_string).collect();
        if !names.is_empty() {
            tmux!["set-option", "-g", SAVED_OPT, join_names(&names)];
        }
    }
    sync_sessions();
}

/// The state as JSON. Only agents with a running claude and a session id.
pub fn snapshot(agents: &[Agent], sidebars: &[String], sessions: &[String]) -> String {
    let agents: Vec<Value> = agents
        .iter()
        .filter_map(|a| {
            let id = a.session_id.as_ref()?;
            // the parent by position: pane ids change after a restore
            let parent = a.parent.as_deref()
                .and_then(|p| agents.iter().find(|x| x.pane == p))
                .map(|x| x.position.clone());
            Some(json!({"position": a.position, "session_id": id, "cwd": a.cwd, "parent": parent}))
        })
        .collect();
    serde_json::to_string_pretty(&json!({"agents": agents, "sidebars": sidebars, "sessions": sessions}))
        .unwrap_or_default()
}

/// Write the state file atomically (temp file + rename).
pub fn save(content: &str) {
    let path = state_file();
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension(format!("json.{}", std::process::id()));
    if std::fs::write(&tmp, content).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// The sidebar was turned on or off in a session: save the session names.
/// Off: the session has no sidebar panes to close after a restore.
pub fn session_toggled(session: &str, on: bool) {
    let sessions = sync_sessions();
    let mut state = std::fs::read_to_string(state_file())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"agents": [], "sidebars": []}));
    state["sessions"] = json!(sessions);
    if !on {
        let prefix = format!("{session}:");
        let kept: Vec<Value> = state["sidebars"].as_array().into_iter().flatten()
            .filter(|p| !p.as_str().is_some_and(|p| p.starts_with(&prefix)))
            .cloned()
            .collect();
        state["sidebars"] = json!(kept);
    }
    save(&serde_json::to_string_pretty(&state).unwrap_or_default());
}

/// `agent-sidebar resume`: run by tmux-resurrect after a restore.
pub fn cmd_resume() {
    tmux!["set-option", "-gu", RESTORING_OPT];
    if gopt("@agent_sidebar_resume") == "off" {
        return;
    }
    let Ok(text) = std::fs::read_to_string(state_file()) else { return };
    let Ok(state) = serde_json::from_str::<Value>(&text) else { return };
    // position -> (pane id, command, cwd, at the left edge with the sidebar width)
    let width = SIDEBAR_WIDTH.to_string();
    let panes: HashMap<String, (String, String, String, bool)> = tmux![
        "list-panes", "-a", "-F",
        "#{session_name}:#{window_index}.#{pane_index}\t#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{pane_left}\t#{pane_width}"
    ]
    .lines()
    .filter_map(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        let [pos, pane, cmd, path, left, w] = f[..] else { return None };
        Some((pos.to_string(), (pane.into(), cmd.into(), path.into(), left == "0" && w == width)))
    })
    .collect();
    let at_shell = |cmd: &str| SHELLS.contains(&cmd);

    let mut resumed = 0;
    for a in state["agents"].as_array().into_iter().flatten() {
        let (Some(pos), Some(id), Some(cwd)) =
            (a["position"].as_str(), a["session_id"].as_str(), a["cwd"].as_str())
        else {
            continue;
        };
        let Some((pane, cmd, path, _)) = panes.get(pos) else { continue };
        // only a restored shell in the same directory, and only a session that still exists
        let transcript = format!("{}/projects/{}/{id}.jsonl", *CLAUDE_DIR, project_dir_name(cwd));
        if !at_shell(cmd) || path != cwd || !std::path::Path::new(&transcript).exists() {
            continue;
        }
        tmux!["send-keys", "-t", pane, "-l", format!("claude --resume {id}")];
        tmux!["send-keys", "-t", pane, "Enter"];
        resumed += 1;
    }
    // the links from an agent to the agent that started it
    for a in state["agents"].as_array().into_iter().flatten() {
        if let (Some(pos), Some(parent)) = (a["position"].as_str(), a["parent"].as_str())
            && let (Some((pane, ..)), Some((parent, ..))) = (panes.get(pos), panes.get(parent))
        {
            tmux!["set-option", "-p", "-t", pane, "@agent_parent", parent];
        }
    }
    // the restored sidebar panes are plain shells now: close them
    for pos in state["sidebars"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some((pane, cmd, _, sidebar_shape)) = panes.get(pos)
            && at_shell(cmd)
            && *sidebar_shape
        {
            tmux!["kill-pane", "-t", pane];
        }
    }
    // turn the sidebars on again: in the saved sessions, or (an older state
    // file without them) in all sessions when an agent was resumed
    let saved = state["sessions"].as_array().map(|names| {
        names.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()
    });
    let mut any = false;
    for line in tmux!["list-sessions", "-F", "#{session_id}\t#{session_name}"].lines() {
        let Some((id, name)) = line.split_once('\t') else { continue };
        if saved.as_ref().map_or(resumed > 0, |s| s.iter().any(|n| n == name)) {
            tmux!["set-option", "-t", id, "@agent_sidebar_on", "1"];
        }
        any |= sidebar_on(id);
    }
    if any {
        if gopt("@agent_sidebar_home").is_empty() {
            tmux!["set-option", "-g", "@agent_sidebar_home", current_session(None)];
        }
        for window in tmux!["list-windows", "-a", "-F", "#{window_id}"].split_whitespace() {
            ensure(window);
        }
    }
    if resumed > 0 {
        tmux!["display-message", format!("{} {resumed}", crate::config::t().resumed)];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_only_running_agents() {
        let mut a = crate::model::test_agent("%1", "agents", None);
        a.position = "agents:1.2".into();
        a.session_id = Some("abc".into());
        let mut b = a.clone();
        b.session_id = None;
        a.cwd = "/repo".into();
        let v: Value =
            serde_json::from_str(&snapshot(&[a, b], &["agents:1.1".into()], &["agents".into()])).unwrap();
        assert_eq!(v["agents"].as_array().unwrap().len(), 1);
        assert_eq!(v["agents"][0]["session_id"], "abc");
        assert_eq!(v["sidebars"][0], "agents:1.1");
        assert_eq!(v["sessions"][0], "agents");
    }

    #[test]
    fn merge_sessions_keeps_gone_sessions() {
        // a: on now; b: off now; c: gone, was on; d: new and on
        let names = merge_sessions(":a:b:c:", "a=1:b=:d=1:");
        assert_eq!(names, ["a", "c", "d"]);
        assert_eq!(join_names(&names), ":a:c:d:");
        assert_eq!(merge_sessions("", "x=:"), Vec::<String>::new());
        assert_eq!(join_names(&[]), "");
    }

    #[test]
    fn snapshot_keeps_the_parent_by_position() {
        let mut p = crate::model::test_agent("%1", "agents", None);
        p.position = "agents:1.1".into();
        p.session_id = Some("p".into());
        let mut c = crate::model::test_agent("%7", "agents", Some("%1"));
        c.position = "agents:3.1".into();
        c.session_id = Some("c".into());
        let v: Value = serde_json::from_str(&snapshot(&[p, c], &[], &[])).unwrap();
        assert_eq!(v["agents"][0]["parent"], Value::Null);
        assert_eq!(v["agents"][1]["parent"], "agents:1.1");
    }
}
