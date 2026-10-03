//! Resume the agents after a tmux restart (tmux-resurrect).
//!
//! The visible sidebar writes the running agents (pane position, Claude Code
//! session id, cwd) and the sidebar pane positions to a state file.
//! tmux-resurrect restores panes as plain shells; its post-restore hook runs
//! `agent-sidebar resume`, which starts `claude --resume <id>` in each agent
//! pane and closes the panes that were sidebars.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::config::SIDEBAR_WIDTH;
use crate::live::CLAUDE_DIR;
use crate::model::Agent;
use crate::panes::ensure;
use crate::tmux::{current_session, gopt};
use crate::transcript::project_dir_name;

const SHELLS: [&str; 9] = ["bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "nu", "elvish"];

pub fn state_file() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| format!("{}/.local/state", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(base).join("tmux-agent-sidebar").join("agents.json")
}

/// The state as JSON. Only agents with a running claude and a session id.
pub fn snapshot(agents: &[Agent], sidebars: &[String]) -> String {
    let agents: Vec<Value> = agents
        .iter()
        .filter_map(|a| {
            let id = a.session_id.as_ref()?;
            Some(json!({"position": a.position, "session_id": id, "cwd": a.cwd}))
        })
        .collect();
    serde_json::to_string_pretty(&json!({"agents": agents, "sidebars": sidebars}))
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

/// The sidebars are off: no sidebar panes to close after a restore.
pub fn forget_sidebars() {
    let Ok(text) = std::fs::read_to_string(state_file()) else { return };
    let Ok(mut state) = serde_json::from_str::<Value>(&text) else { return };
    state["sidebars"] = json!([]);
    save(&serde_json::to_string_pretty(&state).unwrap_or_default());
}

/// `agent-sidebar resume`: run by tmux-resurrect after a restore.
pub fn cmd_resume() {
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
    // the restored sidebar panes are plain shells now: close them
    for pos in state["sidebars"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some((pane, cmd, _, sidebar_shape)) = panes.get(pos)
            && at_shell(cmd)
            && *sidebar_shape
        {
            tmux!["kill-pane", "-t", pane];
        }
    }
    if resumed > 0 {
        // turn the sidebars on again
        tmux!["set-option", "-g", "@agent_sidebar_on", "1"];
        if gopt("@agent_sidebar_home").is_empty() {
            tmux!["set-option", "-g", "@agent_sidebar_home", current_session(None)];
        }
        for window in tmux!["list-windows", "-a", "-F", "#{window_id}"].split_whitespace() {
            ensure(window);
        }
        tmux!["display-message", format!("{} {resumed}", crate::config::t().resumed)];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_only_running_agents() {
        let mut a = crate::model::Agent {
            pane: "%1".into(), position: "agents:1.2".into(), session_id: Some("abc".into()),
            session: "agents".into(), window_id: "@1".into(), order: (1, 2), window: "w".into(),
            name: String::new(), pid: Some(1), hook: "idle".into(), transcript: None,
            cwd: "/x".into(), git: None, tokens: None, status: "idle".into(), ts: None,
        };
        let mut b = a.clone();
        b.session_id = None;
        a.cwd = "/repo".into();
        let v: Value = serde_json::from_str(&snapshot(&[a, b], &["agents:1.1".into()])).unwrap();
        assert_eq!(v["agents"].as_array().unwrap().len(), 1);
        assert_eq!(v["agents"][0]["session_id"], "abc");
        assert_eq!(v["sidebars"][0], "agents:1.1");
    }
}
