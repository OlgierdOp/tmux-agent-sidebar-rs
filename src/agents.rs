//! Commands that let one agent (or a script) start and drive other agents:
//! `spawn`, `prompt`, `list`, `wait`, `read`. See `skills/tmux-agents/SKILL.md`.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::model::{collect_agents, Agent};
use crate::tmux::current_session;
use crate::worktree;

const USAGE: &str = "usage:
  agent-sidebar spawn --name NAME [--branch BRANCH | --no-worktree] [--repo DIR]
                      [--session SESSION] [--prompt TEXT | --prompt-file FILE]
  agent-sidebar prompt AGENT TEXT        (TEXT '-' reads stdin)
  agent-sidebar list [--json]
  agent-sidebar wait AGENT [--until idle,done,waiting] [--timeout SECONDS]
  agent-sidebar read AGENT [--lines N]
AGENT is the agent's name or its tmux pane id (%12).";

fn fail(msg: &str) -> i32 {
    eprintln!("agent-sidebar: {msg}");
    1
}

/// `--flag value` pairs and the other (positional) arguments.
struct Args {
    flags: Vec<(String, Option<String>)>,
    rest: Vec<String>,
}

const BOOL_FLAGS: [&str; 2] = ["--no-worktree", "--json"];

fn parse(args: &[String]) -> Args {
    let (mut flags, mut rest) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a.starts_with("--") && a.len() > 2 {
            let value = (!BOOL_FLAGS.contains(&a.as_str())).then(|| it.next().cloned()).flatten();
            flags.push((a.clone(), value));
        } else {
            rest.push(a.clone());
        }
    }
    Args { flags, rest }
}

impl Args {
    fn get(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|(f, _)| f == name).and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(f, _)| f == name)
    }

    fn unknown(&self, known: &[&str]) -> Option<&str> {
        self.flags.iter().map(|(f, _)| f.as_str()).find(|f| !known.contains(f))
    }
}

/// The agent with this name (the name in the list) or pane id.
fn find(target: &str) -> Option<Agent> {
    let agents = collect_agents(None, &[]);
    if target.starts_with('%') {
        return agents.into_iter().find(|a| a.pane == target);
    }
    agents.into_iter().find(|a| a.label() == target || a.name == target || a.window == target)
}

/// `spawn`: a new tmux window (in the background) with claude, in a new
/// worktree of the repo, with an optional first prompt. Prints JSON.
pub fn cmd_spawn(args: &[String]) -> i32 {
    let a = parse(args);
    if let Some(f) = a.unknown(&["--name", "--branch", "--no-worktree", "--repo", "--session",
                                 "--prompt", "--prompt-file"]) {
        return fail(&format!("unknown option {f}\n{USAGE}"));
    }
    let Some(name) = a.get("--name").filter(|n| !n.is_empty()) else {
        return fail(&format!("--name is required\n{USAGE}"));
    };
    // also a pane that got the name a moment ago and whose claude is still starting
    let named = tmux!["list-panes", "-a", "-F", "#{@agent_name}"].lines().any(|n| n == name);
    if named || find(name).is_some() {
        return fail(&format!("an agent named {name} exists already"));
    }
    let repo = a.get("--repo").map(str::to_string).unwrap_or_else(|| {
        std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    });
    let prompt = match (a.get("--prompt"), a.get("--prompt-file")) {
        (Some(p), _) => Some(p.to_string()),
        (None, Some(f)) => match std::fs::read_to_string(f) {
            Ok(p) => Some(p),
            Err(e) => return fail(&format!("{f}: {e}")),
        },
        _ => None,
    };
    // the directory: a worktree for the branch, or the repo itself
    let (dir, branch) = if a.has("--no-worktree") {
        let top = crate::git::git_info(&repo).map(|g| g.top).unwrap_or(repo.clone());
        (top, None)
    } else {
        let Some(main) = worktree::main_worktree(&repo) else {
            return fail(&format!("{repo} is not in a git repo (use --no-worktree)"));
        };
        let branch = a.get("--branch").unwrap_or(name).to_string();
        match worktree::add(&main, &branch) {
            Ok(path) => (path, Some(branch)),
            Err(e) => return fail(&e),
        }
    };
    // the caller's session, so the new agent shows next to it
    let session = match a.get("--session") {
        Some(s) => s.to_string(),
        None => current_session(std::env::var("TMUX_PANE").ok().as_deref()),
    };
    let out = tmux!["new-window", "-d", "-t", format!("{session}:"), "-c", dir, "-n", name,
                    "-P", "-F", "#{pane_id} #{window_id}"];
    let mut it = out.split_whitespace();
    let (Some(pane), Some(window)) = (it.next(), it.next()) else {
        return fail(&format!("tmux could not open a window in session {session}"));
    };
    tmux!["set-option", "-p", "-t", pane, "@agent_name", name];
    worktree::start_claude(pane, Some(name), prompt.as_deref());
    println!("{}", json!({"name": name, "pane": pane, "window": window, "session": session,
                          "path": dir, "branch": branch}));
    0
}

/// `prompt`: paste the text into the agent's input (bracketed paste, so
/// newlines stay in one message) and press Enter.
pub fn cmd_prompt(args: &[String]) -> i32 {
    let (Some(target), Some(text)) = (args.first(), args.get(1)) else {
        return fail(USAGE);
    };
    let text = if text == "-" {
        let mut s = String::new();
        if std::io::stdin().read_to_string(&mut s).is_err() {
            return fail("cannot read stdin");
        }
        s
    } else {
        text.clone()
    };
    let Some(agent) = find(target) else {
        return fail(&format!("no agent {target}"));
    };
    let buffer = format!("agent-sidebar-{}", std::process::id());
    let loaded = Command::new("tmux")
        .args(["load-buffer", "-b", &buffer, "-"])
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().unwrap().write_all(text.trim_end().as_bytes())?;
            child.wait()
        })
        .is_ok_and(|status| status.success());
    if !loaded {
        return fail("tmux load-buffer failed");
    }
    tmux!["paste-buffer", "-p", "-d", "-b", buffer, "-t", agent.pane];
    // let the input take the paste before Enter submits it
    std::thread::sleep(Duration::from_millis(150));
    tmux!["send-keys", "-t", agent.pane, "Enter"];
    0
}

pub fn cmd_list(args: &[String]) -> i32 {
    let agents = collect_agents(None, &[]);
    if parse(args).has("--json") {
        let list: Vec<_> = agents
            .iter()
            .map(|a| json!({
                "name": a.label(), "pane": a.pane, "session": a.session, "status": a.status,
                "branch": a.git.as_ref().map(|g| g.branch.clone()), "path": a.cwd,
                "tokens": a.tokens,
            }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
        return 0;
    }
    for a in &agents {
        let branch = a.git.as_ref().map_or("-", |g| g.branch.as_str());
        println!("{}\t{}\t{}\t{}\t{}", a.label(), a.status, branch, a.pane, a.cwd);
    }
    0
}

/// `wait`: until the agent's status is one of `--until` (default: idle, done
/// or waiting, i.e. it needs nothing more or it needs you). If the agent is
/// not working yet (a prompt was just sent), it first waits up to 10 s for
/// it to start. Prints the status. Exit 1 on timeout or when the agent is gone.
pub fn cmd_wait(args: &[String]) -> i32 {
    let a = parse(args);
    let Some(target) = a.rest.first() else { return fail(USAGE) };
    let until: Vec<String> = a.get("--until").unwrap_or("idle,done,waiting")
        .split(',').map(|s| s.trim().to_string()).collect();
    let timeout = a.get("--timeout").and_then(|t| t.parse::<f64>().ok()).filter(|t| *t > 0.0)
        .map(Duration::from_secs_f64);
    let start = Instant::now();
    let mut seen_working = false;
    loop {
        let Some(agent) = find(target) else {
            return fail(&format!("no agent {target}"));
        };
        seen_working |= agent.status == "working";
        let starting = !seen_working && start.elapsed() < Duration::from_secs(10);
        if until.contains(&agent.status) && !(starting && agent.status != "waiting") {
            println!("{}", agent.status);
            return 0;
        }
        if timeout.is_some_and(|t| start.elapsed() >= t) {
            println!("{}", agent.status);
            return fail("timeout");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `read`: the last lines of the agent's screen.
pub fn cmd_read(args: &[String]) -> i32 {
    let a = parse(args);
    let Some(target) = a.rest.first() else { return fail(USAGE) };
    let lines = a.get("--lines").and_then(|n| n.parse::<i64>().ok()).unwrap_or(40).max(1);
    let Some(agent) = find(target) else {
        return fail(&format!("no agent {target}"));
    };
    let text = tmux!["capture-pane", "-p", "-J", "-t", agent.pane, "-S", format!("-{lines}")];
    let text: Vec<&str> = text.trim_end().lines().collect();
    let skip = text.len().saturating_sub(lines as usize);
    println!("{}", text[skip..].join("\n"));
    0
}
