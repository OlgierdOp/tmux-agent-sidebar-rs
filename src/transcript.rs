//! Context tokens and the end of the last turn, from the session transcript.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::sync::{LazyLock, Mutex};

use serde_json::Value;

/// Claude Code's project directory name for a cwd.
pub fn project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Fallback without hook data: newest project transcript for this cwd.
pub fn guess_transcript(cwd: &str) -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = format!("{home}/.claude/projects/{}", project_dir_name(cwd));
    let entries = std::fs::read_dir(&dir).ok()?;
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".jsonl"))
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, e.path().to_string_lossy().into_owned()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p)
}

const INTERRUPT_MARK: &[u8] = b"[Request interrupted by user";
const FINISH_REASONS: [&str; 3] = ["end_turn", "stop_sequence", "max_tokens"];
const TAIL_BYTES: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TranscriptInfo {
    /// Context size of the last assistant reply.
    pub tokens: Option<i64>,
    /// Unix time of a "[Request interrupted by user...]" message at the end.
    pub interrupted: Option<f64>,
    /// The last message is an assistant reply that ended the turn.
    pub finished: bool,
}

type CacheKey = (i64, i64, u64); // mtime (s, ns), size
static CACHE: LazyLock<Mutex<HashMap<String, (CacheKey, TranscriptInfo)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Unix time of an ISO 8601 timestamp like "2026-10-03T12:41:13.467Z".
pub fn parse_iso(ts: &str) -> Option<f64> {
    let b = ts.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = ts.get(r)?;
        s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse().ok())?
    };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ')
        || b[13] != b':' || b[16] != b':'
    {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let mut i = 19;
    let mut frac = 0.0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        frac = format!("0.{}", &ts[start..i]).parse().ok()?;
    }
    let offset = match &ts[i..] {
        "" | "Z" => 0, // no zone: taken as UTC
        rest if rest.len() == 6 && (rest.starts_with('+') || rest.starts_with('-')) => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let (oh, om) = (num(i + 1..i + 3)?, num(i + 4..i + 6)?);
            if &rest[3..4] != ":" {
                return None;
            }
            sign * (oh * 3600 + om * 60)
        }
        _ => return None,
    };
    let secs = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s - offset;
    Some(secs as f64 + frac)
}

fn entry_time(entry: &Value) -> Option<f64> {
    parse_iso(entry.get("timestamp")?.as_str()?)
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// (context tokens, interrupt time, finished) from the transcript.
///
/// Tokens come from the last assistant reply. The last message of the main
/// chain tells how the turn ended:
/// - "[Request interrupted by user...]": you pressed Esc or rejected a
///   permission prompt (interrupt time). Claude Code runs no hook for this.
/// - an assistant reply that ended the turn (finished = true).
///
/// Esc before the first reply leaves your prompt as the last message.
pub fn transcript_info(path: Option<&str>) -> TranscriptInfo {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return TranscriptInfo::default();
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return TranscriptInfo::default();
    };
    let key = (meta.mtime(), meta.mtime_nsec(), meta.size());
    if let Some((k, info)) = CACHE.lock().unwrap().get(path)
        && *k == key {
            return *info;
        }
    let info = read_tail(path, meta.size()).unwrap_or_default();
    CACHE.lock().unwrap().insert(path.to_string(), (key, info));
    info
}

fn read_tail(path: &str, size: u64) -> Option<TranscriptInfo> {
    let mut info = TranscriptInfo::default();
    let mut f = File::open(path).ok()?;
    f.seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES))).ok()?;
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return Some(info);
    }
    let mut seen_last = false;
    for line in buf.split(|b| *b == b'\n').rev() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || (seen_last && !contains(line, b"\"usage\"")) {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if !entry.is_object() || truthy(entry.get("isSidechain")) {
            continue;
        }
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "user" && kind != "assistant" {
            continue;
        }
        let message = entry.get("message").filter(|m| m.is_object());
        if !seen_last {
            seen_last = true;
            if kind == "user" && contains(line, INTERRUPT_MARK) {
                info.interrupted = entry_time(&entry);
            }
            info.finished = kind == "assistant"
                && message
                    .and_then(|m| m.get("stop_reason"))
                    .and_then(Value::as_str)
                    .is_some_and(|r| FINISH_REASONS.contains(&r));
        }
        if kind != "assistant" {
            continue;
        }
        let Some(usage) = message.and_then(|m| m.get("usage")).filter(|u| truthy(Some(u))) else {
            continue;
        };
        let tokens = [
            "input_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
            "output_tokens",
        ]
        .iter()
        .map(|k| usage.get(*k).and_then(Value::as_i64).unwrap_or(0))
        .sum();
        info.tokens = Some(tokens);
        break;
    }
    Some(info)
}

/// 950 -> "950", 38_000 -> "38k", 1_200_000 -> "1.2M".
pub fn fmt_tokens(n: Option<i64>) -> String {
    match n {
        None => String::new(),
        Some(n) if n < 1000 => n.to_string(),
        Some(n) if n < 1_000_000 => format!("{}k", n.div_euclid(1000)),
        Some(n) => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0.0));
        let t = parse_iso("2026-10-03T12:41:13.467Z").unwrap();
        assert!((t - 1791031273.467).abs() < 1e-6);
        assert_eq!(parse_iso("2026-10-03T14:41:13+02:00"), Some(1791031273.0));
        assert_eq!(parse_iso("nope"), None);
    }

    #[test]
    fn tokens() {
        assert_eq!(fmt_tokens(None), "");
        assert_eq!(fmt_tokens(Some(950)), "950");
        assert_eq!(fmt_tokens(Some(38_999)), "38k");
        assert_eq!(fmt_tokens(Some(1_250_000)), "1.2M");
    }
}
