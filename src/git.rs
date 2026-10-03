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
}

static CACHE: LazyLock<Mutex<HashMap<String, (Instant, Option<GitInfo>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn clear_cache() {
    CACHE.lock().unwrap().clear();
}

/// `git -C path args...`: trimmed stdout, or None on error or after 2 s.
fn git(path: &str, args: &[&str]) -> Option<String> {
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
