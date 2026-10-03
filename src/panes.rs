//! Sidebar panes: add one to a window, show an agent's window.

use std::time::Duration;

use crate::config::SIDEBAR_WIDTH;
use crate::model::Agent;
use crate::tmux::gopt;

pub fn sidebar_in(window: &str) -> Option<String> {
    tmux!["list-panes", "-t", window, "-F", "#{pane_id}\t#{@agent_sidebar}"]
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find(|(_, flag)| *flag == "1")
        .map(|(pane, _)| pane.to_string())
}

/// Shell command that starts a sidebar pane: this binary, quoted.
fn self_command() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "agent-sidebar".into());
    // after a rebuild, /proc/self/exe of an old process ends in " (deleted)"
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
    format!("'{}'", exe.replace('\'', "'\\''"))
}

/// Add a sidebar to the window (keeps the active pane). Returns its id.
pub fn ensure(window: &str) -> Option<String> {
    if window.is_empty() || gopt("@agent_sidebar_on") != "1" {
        return None;
    }
    if let Some(sb) = sidebar_in(window) {
        return Some(sb);
    }
    let sb = tmux!["split-window", "-d", "-hbf", "-l", SIDEBAR_WIDTH.to_string(), "-t", window,
                   "-P", "-F", "#{pane_id}", self_command()]
        .trim()
        .to_string();
    if sb.is_empty() {
        return None;
    }
    tmux!["set-option", "-p", "-t", sb, "@agent_sidebar", "1"];
    Some(sb)
}

/// Show the agent's window. focus=false: the cursor stays in the sidebar.
pub fn jump(agent: &mut Agent, focus: bool) {
    let pane = agent.pane.clone();
    let window = tmux!["display-message", "-p", "-t", pane, "#{window_id}"].trim().to_string();
    // add the sidebar before the window becomes visible
    let sb = ensure(&window);
    if let Some(sb) = &sb
        && Some(sb.as_str()) != std::env::var("TMUX_PANE").ok().as_deref() {
            // wait (max ~150 ms) until the target window's sidebar draws the new selection
            for _ in 0..15 {
                if tmux!["display-message", "-p", "-t", sb, "#{@agent_sidebar_drawn}"].trim() == pane {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    tmux!["select-window", "-t", window];
    tmux!["select-pane", "-t", pane];
    tmux!["switch-client", "-t", pane];
    if let (Some(sb), false) = (&sb, focus) {
        // the agent stays the "last" pane, so Ctrl+l / prefix ; lands on it
        tmux!["select-pane", "-t", sb];
    }
    tmux!["set-option", "-g", "@agent_sidebar_sel", pane];
    if agent.status == "done" {
        // seen -> no longer highlighted
        tmux!["set-option", "-p", "-t", pane, "@agent_status", "idle"];
        agent.status = "idle".into();
        agent.hook = "idle".into();
    }
}
