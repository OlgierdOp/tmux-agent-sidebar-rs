//! Claude Code's own live state: `~/.claude/sessions/<pid>.json`.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use serde_json::{Map, Value};

use crate::transcript::project_dir_name;

pub type Session = Map<String, Value>;

pub static CLAUDE_DIR: LazyLock<String> = LazyLock::new(|| {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| format!("{}/.claude", std::env::var("HOME").unwrap_or_default()))
});

fn sessions_dir() -> String {
    format!("{}/sessions", *CLAUDE_DIR)
}

/// State in the session file -> sidebar status.
pub fn live_status(state: &str) -> Option<&'static str> {
    match state {
        "busy" => Some("working"),
        "waiting" => Some("waiting"),
        "idle" => Some("idle"),
        _ => None,
    }
}

type Mtime = (i64, i64);
/// path -> (mtime, data)
static SESSION_CACHE: LazyLock<Mutex<HashMap<String, (Mtime, Option<Session>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// background job id -> session file
static JOB_FILES: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn read_session(path: &str) -> Option<Session> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = (meta.mtime(), meta.mtime_nsec());
    let old = SESSION_CACHE.lock().unwrap().get(path).cloned();
    if let Some((m, data)) = &old
        && *m == mtime {
            return data.clone();
        }
    let parsed = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let data = match parsed {
        Some(Value::Object(map)) => Some(map),
        Some(_) => None,
        // file is being rewritten: keep the old data
        None => return old.and_then(|(_, data)| data),
    };
    SESSION_CACHE.lock().unwrap().insert(path.to_string(), (mtime, data.clone()));
    data
}

fn str_field<'a>(data: &'a Session, key: &str) -> Option<&'a str> {
    data.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// What Claude Code writes about the session of this process, or None.
///
/// A pane can run only a client of a background session ("parkedJobId").
/// Then the state is in the session file of the background process.
pub fn session_info(pid: i64) -> Option<Session> {
    let dir = sessions_dir();
    let mut data = read_session(&format!("{dir}/{pid}.json"));
    let job = data.as_ref().and_then(|d| str_field(d, "parkedJobId")).map(str::to_string);
    if let Some(job) = job {
        let known = JOB_FILES.lock().unwrap().get(&job).cloned();
        let mut bg = known.and_then(|p| read_session(&p));
        if bg.as_ref().and_then(|b| str_field(b, "jobId")) != Some(job.as_str()) {
            bg = None;
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .map(|it| {
                    it.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort(); // os.listdir order is arbitrary, a sorted scan is deterministic
            for name in names.iter().filter(|n| n.ends_with(".json")) {
                let path = format!("{dir}/{name}");
                if let Some(d) = read_session(&path)
                    && str_field(&d, "jobId") == Some(job.as_str()) {
                        JOB_FILES.lock().unwrap().insert(job.clone(), path);
                        bg = Some(d);
                        break;
                    }
            }
        }
        if bg.is_some() {
            data = bg;
        }
    }
    let data = data?;
    let state = data.get("status").and_then(Value::as_str)?;
    live_status(state)?;
    Some(data)
}

/// The live state ("busy", "waiting", "idle") of a session from `session_info`.
pub fn state(info: &Session) -> &str {
    info.get("status").and_then(Value::as_str).unwrap_or("")
}

/// Transcript of the live session (a background session has its own).
pub fn live_transcript(info: &Session) -> Option<String> {
    let sid = str_field(info, "sessionId")?;
    let cwd = str_field(info, "cwd")?;
    let path = format!("{}/projects/{}/{sid}.jsonl", *CLAUDE_DIR, project_dir_name(cwd));
    Path::new(&path).exists().then_some(path)
}

/// Sidebar status from the hook status and Claude Code's live state.
pub fn resolve_status(hook: &str, live: &str) -> &'static str {
    let new = live_status(live).unwrap_or("unknown");
    if new == "idle" && hook == "done" {
        return "done"; // finished while you did not look: only the hook knows
    }
    new
}
