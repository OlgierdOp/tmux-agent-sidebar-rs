//! Sounds when an agent starts waiting for you or finishes.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

pub const DEFAULT_WAITING: &str = "/usr/share/sounds/freedesktop/stereo/message-new-instant.oga";
pub const DEFAULT_DONE: &str = "/usr/share/sounds/freedesktop/stereo/complete.oga";

/// First sound player found in PATH.
static PLAYER: LazyLock<Option<&'static str>> = LazyLock::new(|| {
    let path = std::env::var("PATH").unwrap_or_default();
    ["paplay", "pw-play", "aplay"]
        .into_iter()
        .find(|p| path.split(':').any(|dir| Path::new(dir).join(p).is_file()))
});

/// Play a sound file in the background. Does nothing without a player or file.
pub fn play(file: &str) {
    let Some(player) = *PLAYER else { return };
    if file.is_empty() || !Path::new(file).is_file() {
        return;
    }
    let child = Command::new(player)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        // reap it, so it does not stay a zombie
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
