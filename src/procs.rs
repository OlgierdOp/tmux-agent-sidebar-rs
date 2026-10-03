//! Process tree (Linux `/proc`): find the claude process of a pane.

use std::collections::HashMap;
use std::fs;
use std::sync::LazyLock;

/// (comm, argv) of a process, or None if it is gone.
pub fn proc_info(pid: i64) -> Option<(String, Vec<String>)> {
    let comm = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args = cmdline
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    Some((comm.trim().to_string(), args))
}

pub static HAS_CHILDREN_FILE: LazyLock<bool> = LazyLock::new(|| {
    let me = std::process::id();
    fs::metadata(format!("/proc/{me}/task/{me}/children")).is_ok()
});

/// ppid -> [pid], from `process_children()` when the children files are missing.
pub type Children = HashMap<i64, Vec<i64>>;

pub fn child_pids(pid: i64, children: Option<&Children>) -> Vec<i64> {
    if let Some(children) = children {
        return children.get(&pid).cloned().unwrap_or_default();
    }
    let mut out = Vec::new();
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
        return out;
    };
    for task in tasks.flatten() {
        let path = task.path().join("children");
        match fs::read_to_string(path) {
            Ok(s) => out.extend(s.split_whitespace().filter_map(|c| c.parse::<i64>().ok())),
            Err(_) => break,
        }
    }
    out
}

/// Fallback without /proc/PID/task/TID/children: map ppid -> [pid] from all of /proc.
pub fn process_children() -> Children {
    let mut children = Children::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return children;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<i64>() else { continue };
        let Ok(stat) = fs::read_to_string(format!("/proc/{name}/stat")) else {
            continue;
        };
        // comm is in parentheses and may contain spaces
        let Some(close) = stat.rfind(')') else { continue };
        let ppid = stat
            .get(close + 2..)
            .and_then(|rest| rest.split_whitespace().nth(1))
            .and_then(|p| p.parse::<i64>().ok());
        if let Some(ppid) = ppid {
            children.entry(ppid).or_default().push(pid);
        }
    }
    children
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
/// of the pane, not all of /proc.
pub fn claude_pid_in(pane_pid: i64, children: Option<&Children>) -> Option<i64> {
    const DEPTH: usize = 4;
    let mut level = vec![pane_pid];
    for _ in 0..=DEPTH {
        let mut next = Vec::new();
        for &pid in &level {
            if let Some((comm, args)) = proc_info(pid)
                && is_claude(&comm, &args) {
                    return Some(pid);
                }
            next.extend(child_pids(pid, children));
        }
        level = next;
        if level.is_empty() {
            break;
        }
    }
    None
}

pub fn proc_cwd(pid: i64, fallback: &str) -> String {
    match fs::read_link(format!("/proc/{pid}/cwd")) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(_) => fallback.to_string(),
    }
}
