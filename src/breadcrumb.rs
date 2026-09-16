//! A synchronous, fsync'd startup trail.
//!
//! The normal log goes through a buffering background writer, so if the
//! machine wedges the last writes are simply lost — after a hard freeze there
//! is no way to tell whether maplet had started, or how far it got. These
//! markers are written and flushed to disk immediately, one per stage, so the
//! next incident can be answered from evidence instead of inference.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// Record reaching `stage`. Never fails loudly: forensics must not be able to
/// take down the thing it is observing.
pub fn mark(stage: &str) {
    let _ = try_mark(stage);
}

fn try_mark(stage: &str) -> std::io::Result<()> {
    let dir = crate::config::state_dir();
    std::fs::create_dir_all(&dir)?;
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut file = OpenOptions::new().create(true).append(true).open(dir.join("startup.log"))?;
    writeln!(file, "{secs} pid={} {stage}", std::process::id())?;
    file.flush()?;
    // fsync: a buffered marker is no marker at all when the kernel stops.
    file.sync_all()
}

/// Keep the trail from growing without bound; called once at startup.
pub fn trim() {
    let path = crate::config::state_dir().join("startup.log");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > 500 {
        let kept = lines[lines.len() - 200..].join("\n");
        let _ = std::fs::write(&path, format!("{kept}\n"));
    }
}
