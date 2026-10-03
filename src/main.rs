//! tmux agent sidebar: a list of the Claude Code agents running in tmux panes.
//!
//! Every window has its own sidebar pane (no panes move around, so nothing flickers).
//! Shared state (selection, filter, home session) lives in global tmux options.
//!
//! Usage:
//!   agent-sidebar            # TUI of one sidebar pane
//!   agent-sidebar toggle     # turn the sidebars on/off in all windows
//!   agent-sidebar ensure W   # tmux hook: add a sidebar to window W if it has none
//!   agent-sidebar next       # jump to the agent that is waiting for you

#[macro_use]
mod tmux;
mod config;
mod git;
mod live;
mod model;
mod panes;
mod procs;
mod resume;
mod screen;
mod sound;
mod transcript;
mod ui;
mod worktree;

use config::{status_def, t};
use model::collect_agents;
use panes::{ensure, jump, sidebar_in};
use tmux::{current_session, gopt, sidebar_panes};

fn cmd_toggle() {
    if gopt("@agent_sidebar_on") == "1" {
        tmux!["set-option", "-gu", "@agent_sidebar_on"];
        for pane in sidebar_panes() {
            tmux!["kill-pane", "-t", pane];
        }
        resume::forget_sidebars();
        return;
    }
    tmux!["set-option", "-g", "@agent_sidebar_on", "1"];
    tmux!["set-option", "-g", "@agent_sidebar_home", current_session(None)];
    let here = tmux!["display-message", "-p", "#{window_id}"].trim().to_string();
    // all windows at once: each one is resized only this one time
    for window in tmux!["list-windows", "-a", "-F", "#{window_id}"].split_whitespace() {
        ensure(window);
    }
    if let Some(sb) = sidebar_in(&here) {
        tmux!["select-pane", "-t", sb];
    }
}

fn cmd_next() {
    let home = Some(gopt("@agent_sidebar_home"))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| current_session(None));
    let own = current_session(None);
    let mut candidates: Vec<_> = collect_agents(Some(&home), &[])
        .into_iter()
        .filter(|a| a.status == "waiting" || a.status == "done")
        .collect();
    if candidates.is_empty() {
        tmux!["display-message", t().none_waiting];
        return;
    }
    // current session first, then "waiting" before "done", longest waiting first
    candidates.sort_by_key(|a| {
        let priority = status_def(&a.status).map(|d| d.priority).unwrap_or(u8::MAX);
        (a.session != own, priority, a.ts.unwrap_or(0))
    });
    jump(&mut candidates[0], true);
}

/// Two hooks at once may add two sidebars to one window. The older one stays.
fn startup_dedupe() -> bool {
    let me = std::env::var("TMUX_PANE").unwrap_or_default();
    tmux!["set-option", "-p", "-t", me, "@agent_sidebar", "1"];
    let window = tmux!["display-message", "-p", "-t", me, "#{window_id}"].trim().to_string();
    let num = |p: &str| p.trim_start_matches('%').parse::<i64>().unwrap_or(i64::MAX);
    let mine = num(&me);
    !tmux!["list-panes", "-t", window, "-F", "#{pane_id}\t#{@agent_sidebar}"]
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(pane, flag)| *flag == "1" && *pane != me)
        .any(|(pane, _)| num(pane) < mine)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("toggle") => return cmd_toggle(),
        Some("ensure") => {
            ensure(args.get(1).map(String::as_str).unwrap_or(""));
            return;
        }
        Some("next") => return cmd_next(),
        Some("resume") => return resume::cmd_resume(),
        Some(cmd @ ("worktree-new" | "worktree-open" | "worktree-remove")) => {
            let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
            match cmd {
                "worktree-new" => worktree::cmd_new(arg(1), arg(2)),
                "worktree-open" => worktree::cmd_open(arg(1), arg(2), arg(3)),
                _ => worktree::cmd_remove(arg(1), arg(2), arg(3), arg(4) == "--force"),
            }
            return;
        }
        _ => {}
    }
    let set = |v: &str| std::env::var(v).is_ok_and(|s| !s.is_empty());
    if !set("TMUX") || !set("TMUX_PANE") {
        eprintln!("{}", t().outside_tmux);
        std::process::exit(1);
    }
    if !startup_dedupe() {
        return;
    }
    let Ok(_terminal) = ui::Terminal::enter() else {
        std::process::exit(1);
    };
    ui::Ui::new().run();
}
