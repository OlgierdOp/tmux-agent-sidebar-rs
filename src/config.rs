//! Configuration, UI strings and status table.

use std::sync::LazyLock;
use std::time::Duration;

pub const LANG: &str = "en";

/// Width of the sidebar pane. `AGENT_SIDEBAR_WIDTH` overrides it.
pub static SIDEBAR_WIDTH: LazyLock<i64> = LazyLock::new(|| {
    std::env::var("AGENT_SIDEBAR_WIDTH")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(42)
});

/// Default selection background (xterm-256), overridden by `@agent_sidebar_bg`.
pub const SEL_BG: u8 = 23;
/// Backgrounds cycled by the `c` key (live preview in all sidebars).
pub const BG_CHOICES: [u8; 18] = [
    60, 61, 24, 25, 23, 29, 22, 53, 54, 89, 52, 94, 17, 18, 235, 236, 237, 238,
];

pub struct Strings {
    pub title: &'static str,
    pub no_agents: &'static str,
    pub no_git: &'static str,
    pub help: &'static str,
    pub rename: &'static str,
    pub none_waiting: &'static str,
    pub outside_tmux: &'static str,
    pub no_match: &'static str,
    pub wt_branch: &'static str,
    pub wt_menu: &'static str,
    pub wt_remove: &'static str,
    pub wt_force: &'static str,
    pub wt_removed: &'static str,
    pub wt_no_repo: &'static str,
    pub wt_not_linked: &'static str,
    pub resumed: &'static str,
    /// state filter labels: waiting, done, working
    pub filters: [&'static str; 3],
}

impl Strings {
    pub fn filter_name(&self, status: &str) -> &'static str {
        match status {
            "waiting" => self.filters[0],
            "done" => self.filters[1],
            _ => self.filters[2],
        }
    }
}

const EN: Strings = Strings {
    title: "AGENTS",
    no_agents: "No Claude Code agents",
    no_git: "(not a git repo)",
    help: "⏎ show  / find  w d b a filter  c color",
    rename: "agent name:",
    none_waiting: "No agent is waiting",
    outside_tmux: "agent_sidebar: run inside tmux",
    no_match: "No matching agents",
    wt_branch: "worktree branch:",
    wt_menu: "worktrees",
    wt_remove: "remove worktree and close its agent?",
    wt_force: "worktree has changes. remove anyway? (y/n)",
    wt_removed: "worktree removed:",
    wt_no_repo: "worktree: not a git repo",
    wt_not_linked: "not a linked worktree",
    resumed: "agent sidebar: resumed agents:",
    filters: ["waiting", "done", "busy"],
};

/// UI strings per language. Only English for now.
const STRINGS: &[(&str, Strings)] = &[("en", EN)];

/// UI strings for `LANG` (English when `LANG` has no entry).
pub fn t() -> &'static Strings {
    STRINGS
        .iter()
        .find(|(lang, _)| *lang == LANG)
        .map(|(_, s)| s)
        .unwrap_or(&STRINGS[0].1)
}

/// Main loop tick (new data, spinner). Keys wake it at once.
pub const TICK: Duration = Duration::from_millis(100);
/// Safety-net polling of tmux state. Window, session and pane switches do not
/// wait for it: the tmux hooks wake the sidebar with F12 at once.
pub const POLL: Duration = Duration::from_secs(1);
pub const HIDDEN_POLL: Duration = Duration::from_secs(2);
/// Data refresh while the pane is visible ...
pub const REFRESH: Duration = Duration::from_secs(1);
/// ... and while it is hidden.
pub const HIDDEN_REFRESH: Duration = Duration::from_secs(5);
pub const GIT_TTL: Duration = Duration::from_secs(3);
/// Session order in `@agent_sidebar_order`. tmux does not allow ":" in session names.
pub const ORDER_SEP: &str = ":";
/// How long a just-ended turn waits for its last transcript entry.
pub const FINISH_WAIT: Duration = Duration::from_secs(1);
pub const SEP: &str = "\t";

/// Colors of the palette (curses color pairs in the Python version).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Text,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
}

pub struct StatusDef {
    pub symbol: &'static str,
    pub color: Col,
    /// Priority for `next`: lower comes first.
    pub priority: u8,
}

/// status -> (symbol, color, priority for `next`)
pub fn status_def(status: &str) -> Option<StatusDef> {
    let (symbol, color, priority) = match status {
        "waiting" => ("●", Col::Red, 0), // needs your approval / answer
        "done" => ("●", Col::Green, 1),  // finished, not looked at yet
        "working" => ("◐", Col::Yellow, 2),
        "idle" => ("○", Col::Gray, 3),
        "unknown" => ("?", Col::Gray, 4), // no hook data
        _ => return None,
    };
    Some(StatusDef { symbol, color, priority })
}

pub const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];
