//! tmux commands.

use std::process::{Command, Stdio};

/// Run tmux and return its stdout ("" when tmux cannot run).
pub fn tmux<S: AsRef<str>>(args: &[S]) -> String {
    match Command::new("tmux")
        .args(args.iter().map(|a| a.as_ref()))
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

/// Run tmux without waiting for it. For `command-prompt`, `display-menu` and
/// `confirm-before`: they return only when you close the prompt, and the
/// sidebar must not freeze meanwhile.
pub fn tmux_bg<S: AsRef<str>>(args: &[S]) {
    let child = Command::new("tmux")
        .args(args.iter().map(|a| a.as_ref()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        std::thread::spawn(move || {
            let _ = child.wait(); // reap it
        });
    }
}

/// Shorthand: `tmux!["set-option", "-g", name, value]`.
#[macro_export]
macro_rules! tmux {
    ($($arg:expr),* $(,)?) => {
        $crate::tmux::tmux(&[$(::std::convert::AsRef::<str>::as_ref(&$arg)),*])
    };
}

/// A global tmux option.
pub fn gopt(name: &str) -> String {
    tmux!["show-option", "-gqv", name].trim().to_string()
}

pub fn current_session(pane: Option<&str>) -> String {
    match pane {
        Some(p) => tmux!["display-message", "-p", "-t", p, "#{session_name}"],
        None => tmux!["display-message", "-p", "#{session_name}"],
    }
    .trim()
    .to_string()
}

/// Sidebar panes (pane ids) of the whole server.
pub fn sidebar_panes() -> Vec<String> {
    parse_sidebars(&tmux!["list-panes", "-a", "-F", "#{pane_id}\t#{@agent_sidebar}"])
}

/// Sidebar panes (pane ids) in the windows of one session.
pub fn sidebar_panes_in(session: &str) -> Vec<String> {
    parse_sidebars(&tmux!["list-panes", "-s", "-t", session, "-F", "#{pane_id}\t#{@agent_sidebar}"])
}

fn parse_sidebars(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|line| {
            let (pane, flag) = line.split_once('\t')?;
            (flag == "1").then(|| pane.to_string())
        })
        .collect()
}

/// The sidebar is on in the session of this target (session, window or pane).
/// `@agent_sidebar_on` is a session option: each session has its own.
pub fn sidebar_on(target: &str) -> bool {
    tmux!["display-message", "-p", "-t", target, "#{@agent_sidebar_on}"].trim() == "1"
}
