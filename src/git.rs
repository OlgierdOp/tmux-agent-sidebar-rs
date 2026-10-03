//! Git branch and worktree of a directory, briefly cached.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::config::GIT_TTL;

#[derive(Clone, Debug, PartialEq)]
pub struct GitInfo {
    pub top: String,
    pub branch: String,
    pub linked_worktree: bool,
    pub status: GitStatus,
}

/// Commits ahead of / behind the upstream, and changed files (with untracked).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GitStatus {
    pub ahead: u32,
    pub behind: u32,
    pub changes: u32,
}

type Cache<T> = LazyLock<Mutex<HashMap<String, (Instant, T)>>>;

/// path -> git info
static CACHE: Cache<Option<GitInfo>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// repo top -> status, so agents in one repo share one `git status`
static STATUS_CACHE: Cache<GitStatus> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn clear_cache() {
    CACHE.lock().unwrap().clear();
    STATUS_CACHE.lock().unwrap().clear();
}

/// Parse `git status --porcelain=v2 --branch`.
fn parse_status(out: &str) -> GitStatus {
    let mut st = GitStatus::default();
    for line in out.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            st.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok()).unwrap_or(0);
            st.behind = it.next().and_then(|b| b.trim_start_matches('-').parse().ok()).unwrap_or(0);
        } else if !line.starts_with('#') && !line.is_empty() {
            st.changes += 1;
        }
    }
    st
}

fn git_status(top: &str) -> GitStatus {
    let now = Instant::now();
    if let Some((at, st)) = STATUS_CACHE.lock().unwrap().get(top)
        && now.duration_since(*at) < GIT_TTL
    {
        return *st;
    }
    let st = git(top, &["status", "--porcelain=v2", "--branch"])
        .map(|out| parse_status(&out))
        .unwrap_or_default();
    STATUS_CACHE.lock().unwrap().insert(top.to_string(), (now, st));
    st
}

/// `git -C path args...`: trimmed stdout, or None on error or after 2 s.
pub fn git(path: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    status.success().then(|| out.trim().to_string())
}

fn realpath(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string())
}

/// (toplevel, branch, is_linked_worktree) or None, briefly cached.
pub fn git_info(path: &str) -> Option<GitInfo> {
    let now = Instant::now();
    if let Some((at, info)) = CACHE.lock().unwrap().get(path)
        && now.duration_since(*at) < GIT_TTL {
            return info.clone();
        }
    let mut info = None;
    let out = git(path, &[
        "rev-parse", "--show-toplevel", "--absolute-git-dir",
        "--path-format=absolute", "--git-common-dir",
    ]);
    if let Some(out) = out.filter(|o| !o.is_empty()) {
        let lines: Vec<&str> = out.lines().collect();
        if let [top, git_dir, common] = lines[..] {
            let branch = git(path, &["symbolic-ref", "--short", "-q", "HEAD"])
                .filter(|b| !b.is_empty())
                .or_else(|| git(path, &["rev-parse", "--short", "HEAD"]).filter(|b| !b.is_empty()))
                .unwrap_or_else(|| "?".to_string());
            info = Some(GitInfo {
                top: top.to_string(),
                branch,
                linked_worktree: realpath(git_dir) != realpath(common),
                status: git_status(top),
            });
        }
    }
    CACHE.lock().unwrap().insert(path.to_string(), (now, info.clone()));
    info
}

/// Python-style `s[start:]` on characters (negative start counts from the end).
fn chars_from(s: &str, start: i64) -> String {
    let n = s.chars().count() as i64;
    let start = if start < 0 { (n + start).max(0) } else { start.min(n) };
    s.chars().skip(start as usize).collect()
}

/// `~` for the home directory. Paths longer than `width` keep their end.
pub fn short_path(p: &str, width: Option<i64>) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut p = if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    };
    if let Some(width) = width.filter(|w| *w != 0)
        && p.chars().count() as i64 > width {
            // the end of the path matters most
            p = format!("…{}", chars_from(&p, -(width - 1)));
        }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status() {
        let out = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n\
                   # branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b src/x.rs\n? new.txt\n";
        assert_eq!(parse_status(out), GitStatus { ahead: 2, behind: 1, changes: 2 });
        assert_eq!(parse_status("# branch.head main\n"), GitStatus::default());
    }

    #[test]
    fn paths() {
        assert_eq!(chars_from("abcdef", -3), "def");
        assert_eq!(chars_from("abc", 0), "abc");
    }
}
