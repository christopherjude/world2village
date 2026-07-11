//! Minimal append-to-file logging for `village-service`.
//!
//! A Windows Service has no console, so `eprintln!`/`println!` go nowhere
//! useful once running as `run` (they're only visible when invoked
//! interactively, e.g. during `install`). This gives every code path --
//! including the one-time elevated `install` run, which is exactly the
//! path that had no diagnostics available when `ShellExecuteW` failures
//! needed debugging on a real machine -- somewhere durable to write to.
//!
//! Deliberately not a logging framework (`log`/`tracing`/etc.): this
//! service emits a handful of lifecycle lines, not a stream needing
//! levels, filtering, or structured fields. A dead-simple
//! open-append-write-close per call is more than adequate at this call
//! volume and avoids pulling in a dependency (and its own configuration
//! surface) for what a single function covers.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::install::program_data_dir;

/// `%ProgramData%\Village\service.log` -- lives next to (but is not
/// inside) `install::install_bin_dir()`'s `bin` subdirectory, since it
/// needs to exist and be writable before `bin` itself has necessarily been
/// created (e.g. logging the very start of `run_install`).
fn log_path() -> PathBuf {
    program_data_dir().join("Village").join("service.log")
}

/// Appends a single timestamped line to the service log, creating
/// `%ProgramData%\Village\` first if this is the very first time anything
/// has been logged (e.g. before `install::run_install` has created `bin`
/// itself). Best-effort: a failure to write a log line (e.g. permissions,
/// disk full) is not something any caller here can meaningfully react to,
/// so it's silently swallowed rather than propagated -- logging must never
/// be the reason a real operation fails.
pub fn log(msg: impl std::fmt::Display) {
    let path = log_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = writeln!(file, "[{timestamp}] {msg}");
}
