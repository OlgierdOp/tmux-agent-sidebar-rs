//! Agents: one per tmux pane that runs Claude Code.

use std::collections::HashMap;

use serde_json::Value;

use crate::config::{status_def, SEP};
use crate::git::{git_info, GitInfo};
use crate::live::{live_transcript, resolve_status, session_info, state};
use crate::procs::{claude_pid_in, proc_cwd, Tree};
use crate::transcript::{guess_transcript, transcript_info};

const PANE_FIELDS: [&str; 17] = [
    "pane_id", "pane_pid", "session_name", "window_id", "window_index", "window_name",
    "pane_index", "pane_title", "pane_current_path", "pane_active",
    "window_active",
    "@agent_status", "@agent_ts", "@agent_name", "@agent_sidebar",
    "@agent_transcript", "automatic-rename",
];

type Pane = HashMap<&'static str, String>;

fn list_panes() -> Vec<Pane> {
    let fmt = PANE_FIELDS
        .iter()
        .map(|f| format!("#{{{f}}}"))
        .collect::<Vec<_>>()
        .join(SEP);
    tmux!["list-panes", "-a", "-F", fmt]
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(SEP).collect();
            (parts.len() == PANE_FIELDS.len()).then(|| {
                PANE_FIELDS.iter().copied().zip(parts.iter().map(|p| p.to_string())).collect()
            })
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Agent {
    pub pane: String,
    /// `session:window.pane`, for the resume file
    pub position: String,
    /// Claude Code session id, for `claude --resume`
    pub session_id: Option<String>,
    pub session: String,
    pub window_id: String,
    /// (window index, pane index)
    pub order: (i64, i64),
    pub window: String,
    pub name: String,
    pub pid: Option<i64>,
    /// Status from the hook (tmux option), before the live state.
    pub hook: String,
    pub transcript: Option<String>,
    pub cwd: String,
    pub git: Option<GitInfo>,
    pub tokens: Option<i64>,
    pub status: String,
    pub ts: Option<i64>,
}

impl Agent {
    /// Name shown in the list.
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.window } else { &self.name }
    }
}

fn nonempty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

pub fn collect_agents(home_session: Option<&str>, order: &[String]) -> Vec<Agent> {
    collect(home_session, order).0
}

/// Position of a pane: `session:window.pane`.
fn position(p: &Pane) -> String {
    format!("{}:{}.{}", p["session_name"], p["window_index"], p["pane_index"])
}

/// The agents, and the positions of the sidebar panes.
pub fn collect(home_session: Option<&str>, order: &[String]) -> (Vec<Agent>, Vec<String>) {
    let tree = Tree::new();
    let mut agents = Vec::new();
    let mut sidebars = Vec::new();
    for p in list_panes() {
        if !p["@agent_sidebar"].is_empty() {
            sidebars.push(position(&p));
            continue;
        }
        let Ok(pane_pid) = p["pane_pid"].parse::<i64>() else { continue };
        let pid = claude_pid_in(pane_pid, &tree);
        if pid.is_none() && p["@agent_status"].is_empty() {
            continue;
        }
        let cwd = match pid {
            Some(pid) => proc_cwd(pid, &p["pane_current_path"]),
            None => p["pane_current_path"].clone(),
        };
        let mut status = nonempty(&p["@agent_status"]).unwrap_or_else(|| "unknown".into());
        if pid.is_none() && status != "unknown" {
            status = "idle".into();
        }
        let mut ts = p["@agent_ts"].trim().parse::<i64>().ok();
        let hook = status.clone();
        let info = pid.and_then(session_info);
        let transcript = info
            .as_ref()
            .and_then(live_transcript)
            .or_else(|| nonempty(&p["@agent_transcript"]))
            .or_else(|| guess_transcript(&cwd));
        let tinfo = transcript_info(transcript.as_deref());
        if let Some(info) = &info {
            // Claude Code's own state is exact, also after Esc or Ctrl+C
            let new = resolve_status(&hook, state(info));
            if new != status {
                status = new.to_string();
                let updated = info.get("statusUpdatedAt").and_then(Value::as_f64).unwrap_or(0.0);
                let secs = (updated / 1000.0) as i64;
                if secs != 0 {
                    ts = Some(secs);
                }
            }
        } else if let Some(interrupted) = tinfo.interrupted
            && (status == "working" || status == "waiting")
                && ts.is_none_or(|ts| interrupted >= (ts + 1) as f64)
            {
                // older Claude Code without session files: interrupted or rejected,
                // no hook runs, so store the change here
                status = "idle".into();
                let t = interrupted as i64;
                ts = Some(t);
                let pane = &p["pane_id"];
                tmux!["set-option", "-p", "-t", pane, "@agent_status", status, ";",
                      "set-option", "-p", "-t", pane, "@agent_ts", t.to_string()];
            }
        // the id that `claude --resume` takes (only while claude runs here)
        let session_id = pid.and(
            info.as_ref()
                .and_then(|i| i.get("sessionId").and_then(Value::as_str).map(str::to_string))
                .or_else(|| {
                    let t = transcript.as_deref()?;
                    Some(t.rsplit('/').next()?.strip_suffix(".jsonl")?.to_string())
                }),
        );
        let git = git_info(&cwd);
        let mut name = p["@agent_name"].clone();
        if name.is_empty() && p["automatic-rename"] == "1" {
            // the automatic window name is the command of the active pane
            // (claude, python3, ...), so use the repo or directory name
            let base = match &git {
                Some(g) => g.top.trim_end_matches('/'),
                None => cwd.trim_end_matches('/'),
            };
            name = nonempty(basename(base)).unwrap_or_else(|| cwd.clone());
        }
        let (Ok(wi), Ok(pi)) = (p["window_index"].parse(), p["pane_index"].parse()) else {
            continue;
        };
        agents.push(Agent {
            pane: p["pane_id"].clone(),
            position: position(&p),
            session_id,
            session: p["session_name"].clone(),
            window_id: p["window_id"].clone(),
            order: (wi, pi),
            window: p["window_name"].clone(),
            name,
            pid,
            hook,
            transcript,
            cwd,
            git,
            tokens: tinfo.tokens,
            status: if status_def(&status).is_some() { status } else { "unknown".into() },
            ts,
        });
    }
    sort_agents(&mut agents, home_session, order);
    (agents, sidebars)
}

/// Sessions in your order (J/K, `@agent_sidebar_order`). Sessions not in it
/// follow: the home session first, then the others alphabetically.
pub fn sort_agents(agents: &mut [Agent], home_session: Option<&str>, order: &[String]) {
    let rank: HashMap<&str, usize> =
        order.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    agents.sort_by(|a, b| {
        let key = |x: &Agent| {
            let r = rank.get(x.session.as_str()).copied().unwrap_or(
                rank.len() + usize::from(Some(x.session.as_str()) != home_session),
            );
            (r, x.session.clone(), x.order)
        };
        key(a).cmp(&key(b))
    });
}
