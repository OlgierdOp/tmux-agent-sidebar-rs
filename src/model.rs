//! Agents: one per tmux pane that runs Claude Code.

use std::collections::HashMap;

use serde_json::Value;

use crate::config::{status_def, SEP};
use crate::git::{git_info, GitInfo};
use crate::live::{live_transcript, resolve_status, session_info, state};
use crate::procs::{claude_pid_in, proc_cwd, Tree};
use crate::transcript::{guess_transcript, transcript_info};

const PANE_FIELDS: [&str; 18] = [
    "pane_id", "pane_pid", "session_name", "window_id", "window_index", "window_name",
    "pane_index", "pane_title", "pane_current_path", "pane_active",
    "window_active",
    "@agent_status", "@agent_ts", "@agent_name", "@agent_sidebar",
    "@agent_transcript", "automatic-rename", "@agent_parent",
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
    /// pane of the agent that started this one (`spawn` sets `@agent_parent`)
    pub parent: Option<String>,
    /// depth in the tree of agents: 0 at the top, 1 for a child (set by `sort_agents`)
    pub depth: usize,
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
            parent: nonempty(&p["@agent_parent"]),
            depth: 0,
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
    nest(agents);
}

/// The parent of an agent in the list: an agent of the same session.
fn parent_index(agents: &[Agent], i: usize) -> Option<usize> {
    let parent = agents[i].parent.as_deref()?;
    (0..agents.len()).find(|&j| j != i && agents[j].pane == parent
                               && agents[j].session == agents[i].session)
}

/// Put each agent's children right after it (in their order) and set the
/// depth. An agent whose parent is gone, or in another session, is at the top.
fn nest(agents: &mut [Agent]) {
    let n = agents.len();
    let parents: Vec<Option<usize>> = (0..n).map(|i| parent_index(agents, i)).collect();
    let mut order: Vec<(usize, usize)> = Vec::with_capacity(n); // (index, depth)
    let mut done = vec![false; n];
    fn visit(i: usize, depth: usize, parents: &[Option<usize>], done: &mut [bool],
             order: &mut Vec<(usize, usize)>) {
        done[i] = true;
        order.push((i, depth));
        for c in 0..parents.len() {
            if parents[c] == Some(i) && !done[c] {
                visit(c, depth + 1, parents, done, order);
            }
        }
    }
    for i in 0..n {
        if parents[i].is_none() {
            visit(i, 0, &parents, &mut done, &mut order);
        }
    }
    // a parent loop (no agent of it is at the top): start at its first agent
    for i in 0..n {
        if !done[i] {
            visit(i, 0, &parents, &mut done, &mut order);
        }
    }
    let sorted: Vec<Agent> = order
        .iter()
        .map(|&(i, depth)| Agent { depth, ..agents[i].clone() })
        .collect();
    agents.clone_from_slice(&sorted);
}

/// Agent `i` and the agents below it in the tree (`agents` in tree order).
pub fn subtree(agents: &[Agent], i: usize) -> std::ops::Range<usize> {
    let d = agents[i].depth;
    let n = agents[i + 1..].iter().take_while(|a| a.depth > d).count();
    i..i + 1 + n
}

/// The window with the highest index among the agent `pane` and the agents
/// below it, in `session`: a new child's window goes after it.
pub fn last_window(agents: &[Agent], pane: &str, session: &str) -> Option<String> {
    let i = agents.iter().position(|a| a.pane == pane && a.session == session)?;
    agents[subtree(agents, i)].iter().max_by_key(|a| a.order.0).map(|a| a.window_id.clone())
}

/// `swap-window` pairs that put the windows `want` (window ids) in this
/// order. They keep the indices they have now (`index`), only in another
/// order. Also returns the new index of each window.
pub fn window_swaps(want: &[String], index: &HashMap<String, i64>)
                    -> (Vec<(String, String)>, HashMap<String, i64>) {
    let mut slots: Vec<i64> = want.iter().filter_map(|w| index.get(w).copied()).collect();
    slots.sort();
    let mut pos: HashMap<String, i64> =
        want.iter().filter_map(|w| Some((w.clone(), *index.get(w)?))).collect();
    let mut at: HashMap<i64, String> = pos.iter().map(|(w, i)| (*i, w.clone())).collect();
    let mut swaps = Vec::new();
    for (w, slot) in want.iter().filter(|w| index.contains_key(*w)).zip(slots) {
        let now = pos[w];
        if now == slot {
            continue;
        }
        let other = at[&slot].clone();
        swaps.push((w.clone(), other.clone()));
        pos.insert(w.clone(), slot);
        pos.insert(other.clone(), now);
        at.insert(slot, w.clone());
        at.insert(now, other);
    }
    (swaps, pos)
}

#[cfg(test)]
pub fn test_agent(pane: &str, session: &str, parent: Option<&str>) -> Agent {
    Agent {
        pane: pane.into(), position: String::new(), session_id: None, session: session.into(),
        window_id: String::new(), order: (0, 0), window: pane.into(), name: String::new(),
        pid: None, hook: "idle".into(), transcript: None, cwd: String::new(), git: None,
        tokens: None, status: "idle".into(), ts: None, parent: parent.map(str::to_string),
        depth: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(agents: &[Agent]) -> Vec<(String, usize)> {
        agents.iter().map(|a| (a.pane.clone(), a.depth)).collect()
    }

    #[test]
    fn children_follow_their_parent() {
        let mut agents = vec![
            test_agent("%1", "s", None),
            test_agent("%2", "s", None),
            test_agent("%3", "s", Some("%1")),
            test_agent("%4", "s", Some("%3")),
            test_agent("%5", "s", Some("%1")),
        ];
        for (i, a) in agents.iter_mut().enumerate() {
            a.order = (i as i64, 0);
        }
        sort_agents(&mut agents, Some("s"), &[]);
        let want = [("%1", 0), ("%3", 1), ("%4", 2), ("%5", 1), ("%2", 0)];
        assert_eq!(tree(&agents), want.map(|(p, d)| (p.to_string(), d)));
    }

    #[test]
    fn swaps_put_windows_in_order() {
        let index: HashMap<String, i64> =
            [("@a", 1), ("@b", 2), ("@c", 3), ("@d", 5)].map(|(w, i)| (w.to_string(), i)).into();
        let want: Vec<String> = ["@c", "@d", "@a", "@b"].map(String::from).into();
        let (swaps, pos) = window_swaps(&want, &index);
        // replay the swaps
        let mut at: HashMap<i64, String> = index.iter().map(|(w, i)| (*i, w.clone())).collect();
        for (x, y) in &swaps {
            let ix = *at.iter().find(|(_, w)| *w == x).unwrap().0;
            let iy = *at.iter().find(|(_, w)| *w == y).unwrap().0;
            at.insert(ix, y.clone());
            at.insert(iy, x.clone());
        }
        let mut got: Vec<(i64, String)> = at.into_iter().collect();
        got.sort();
        assert_eq!(got.into_iter().map(|(_, w)| w).collect::<Vec<_>>(), want);
        assert_eq!(pos["@c"], 1);
        assert_eq!(pos["@b"], 5);
        let (none, _) = window_swaps(&["@a".to_string(), "@b".to_string()], &index);
        assert!(none.is_empty());
    }

    #[test]
    fn last_window_of_a_subtree() {
        let mut agents = vec![
            test_agent("%1", "s", None),
            test_agent("%2", "s", Some("%1")),
            test_agent("%3", "s", None),
        ];
        for (i, a) in agents.iter_mut().enumerate() {
            a.order = (i as i64 + 1, 0);
            a.window_id = format!("@{}", i + 1);
        }
        sort_agents(&mut agents, Some("s"), &[]);
        assert_eq!(last_window(&agents, "%1", "s").as_deref(), Some("@2"));
        assert_eq!(last_window(&agents, "%3", "s").as_deref(), Some("@3"));
        assert_eq!(last_window(&agents, "%1", "other"), None);
    }

    #[test]
    fn gone_or_foreign_parent_is_top_level() {
        let mut agents = vec![
            test_agent("%1", "a", None),
            test_agent("%2", "b", Some("%1")),
            test_agent("%3", "a", Some("%9")),
        ];
        nest(&mut agents);
        assert!(agents.iter().all(|a| a.depth == 0));
    }

    #[test]
    fn parent_loop_does_not_hang() {
        let mut agents = vec![test_agent("%1", "s", Some("%2")), test_agent("%2", "s", Some("%1"))];
        nest(&mut agents);
        assert_eq!(tree(&agents), vec![("%1".to_string(), 0), ("%2".to_string(), 1)]);
    }
}
