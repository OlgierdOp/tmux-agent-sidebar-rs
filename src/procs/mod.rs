//! Process tree: find the claude process of a pane.
//! The OS backends (`linux`: `/proc`, `macos`: libproc) give the process
//! name, argv, children and cwd. The search is the same on both.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as os;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("agent-sidebar supports Linux and macOS only");

/// A view of the process tree for one refresh. Make one per refresh:
/// on some systems it holds a snapshot of all processes.
pub use os::Tree;

pub fn proc_cwd(pid: i64, fallback: &str) -> String {
    os::cwd(pid).unwrap_or_else(|| fallback.to_string())
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

pub fn is_claude(comm: &str, args: &[String]) -> bool {
    if comm == "claude" {
        return true;
    }
    args.iter()
        .take(2)
        .any(|a| basename(a) == "claude" || a.contains("claude-code"))
}

/// The claude process in a pane's process tree. Reads only the descendants
/// of the pane, not all processes.
pub fn claude_pid_in(pane_pid: i64, tree: &Tree) -> Option<i64> {
    const DEPTH: usize = 4;
    let mut level = vec![pane_pid];
    for _ in 0..=DEPTH {
        let mut next = Vec::new();
        for &pid in &level {
            if let Some((comm, args)) = os::proc_info(pid)
                && is_claude(&comm, &args) {
                    return Some(pid);
                }
            next.extend(tree.children(pid));
        }
        level = next;
        if level.is_empty() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_claude_by_name_or_argv() {
        assert!(is_claude("claude", &[]));
        // macOS: the process name is the version file, argv[0] is "claude"
        assert!(is_claude("2.1.289", &["claude".into(), "--resume".into()]));
        assert!(is_claude("node", &["node".into(), "/x/@anthropic-ai/claude-code/cli.js".into()]));
        assert!(!is_claude("bash", &["bash".into()]));
    }

    #[test]
    fn reads_own_process() {
        let me = std::process::id() as i64;
        let (comm, args) = os::proc_info(me).expect("own process info");
        assert!(!comm.is_empty());
        assert!(!args.is_empty());
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(proc_cwd(me, ""), cwd.to_string_lossy());
    }

    #[test]
    fn lists_children() {
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let me = std::process::id() as i64;
        let found = Tree::new().children(me).contains(&(child.id() as i64));
        child.kill().ok();
        child.wait().ok();
        assert!(found);
    }
}
