//! Linux backend: `/proc`.

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

pub fn cwd(pid: i64) -> Option<String> {
    fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

static HAS_CHILDREN_FILE: LazyLock<bool> = LazyLock::new(|| {
    let me = std::process::id();
    fs::metadata(format!("/proc/{me}/task/{me}/children")).is_ok()
});

/// ppid -> [pid]
type Children = HashMap<i64, Vec<i64>>;

/// Reads `/proc/PID/task/TID/children`. Without these files (kernel option),
/// a full `/proc` scan at creation.
pub struct Tree(Option<Children>);

impl Tree {
    pub fn new() -> Self {
        Tree((!*HAS_CHILDREN_FILE).then(process_children))
    }

    pub fn children(&self, pid: i64) -> Vec<i64> {
        if let Some(children) = &self.0 {
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
}

/// Fallback without /proc/PID/task/TID/children: map ppid -> [pid] from all of /proc.
fn process_children() -> Children {
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
