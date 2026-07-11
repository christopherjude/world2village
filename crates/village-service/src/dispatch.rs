//! Turns one IPC `Request` into a `Response`, against shared session state
//! for the current (at most one) `edge.exe` child process.
//!
//! # Security model
//!
//! - `edge.exe` is always spawned from the fixed path
//!   `install::edge_exe_path()` (`%ProgramData%\Village\bin\edge.exe`) --
//!   **never** from a path carried in the request. This is the concrete
//!   defense against a compromised/malicious low-privilege IPC client
//!   trying to get this SYSTEM-elevated service to execute an arbitrary
//!   binary by redirecting a path.
//! - Every `StartProfile` request's `ResolvedProfile` (plain strings/
//!   primitives, as defined in `village_ipc::protocol` -- that crate
//!   deliberately does not depend on `village_core`'s validated newtypes)
//!   is re-validated here, from scratch, through `village_core`'s
//!   constructors (`Community::new`, `PassKey::new`, `SupernodeAddr::new`,
//!   a hand-rolled MAC parse mirroring `MacAddr`'s `Display` format)
//!   before being turned into argv via `village_core::argv::build_edge_argv`.
//!   The service never assumes data that already crossed the IPC boundary
//!   is safe just because the GUI validated it on its side first.
//! - `Request` is a closed, fixed set of operations
//!   (`village_ipc::protocol::Request`, `#[serde(deny_unknown_fields)]`) --
//!   there is no free-form command execution path here.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use village_core::argv::{build_edge_argv, EdgeArgs};
use village_core::mac::MacAddr;
use village_core::profile::{AdvancedSettings, Cipher, Community, Compression, PassKey, SupernodeAddr};
use village_ipc::protocol::{ConnectionStatus, ErrorCode, Request, ResolvedProfile, Response};

use crate::install;
use crate::job::EdgeJob;
use crate::logging::log;

/// How long to wait for `edge.exe` to report an assigned overlay IP on its
/// stdout before giving up and transitioning to `Error` instead of hanging
/// in `Starting` forever.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

/// The service's view of the current `edge.exe` session. At most one
/// session is ever active at a time, matching the product's single-active-
/// connection model.
enum Session {
    Idle,
    /// Spawned, waiting to see a recognizable "assigned IP" line on stdout
    /// (or for `STARTUP_TIMEOUT` to elapse).
    Starting { child: Child },
    /// Reported an overlay IP.
    Connected {
        child: Child,
        overlay_ip: String,
        since_unix_secs: u64,
    },
    /// Failed to start, exited unexpectedly, or timed out waiting for an
    /// IP. No child handle is kept here -- whichever code transitions into
    /// this state is responsible for having already killed/reaped it.
    Error { message: String },
}

/// Shared session state, held behind `Arc<Mutex<_>>` so both the pipe
/// server's per-connection threads and background stdout-watcher threads
/// can read/update it.
pub struct ServiceState {
    session: Session,
    /// Bumped every time `StartProfile` begins a new session. Lets a
    /// stale background watcher thread from a *previous* session (e.g.
    /// one still blocked reading a since-killed child's stdout) recognize
    /// that it no longer owns the current session and should not touch
    /// `session` when it finally wakes up.
    generation: u64,
    /// Kill-on-close Job Object that every spawned `edge.exe` child is
    /// assigned to (see `start_profile`), so it cannot outlive this service
    /// process -- see `job`'s module doc comment for the orphaned-process
    /// bug this fixes. Created once, here, and reused across every
    /// `StartProfile`/`Stop` cycle for the life of the service process,
    /// since only one `edge.exe` child is ever running at a time.
    ///
    /// `None` if creating the job object itself failed (logged in `new`) --
    /// in that case `edge.exe` is still spawned normally, just without this
    /// extra hardening, rather than refusing to connect at all over a
    /// job-object setup failure.
    job: Option<EdgeJob>,
}

impl ServiceState {
    pub fn new() -> Self {
        let job = match EdgeJob::create() {
            Ok(job) => Some(job),
            Err(err) => {
                log(format!(
                    "dispatch: failed to create edge.exe kill-on-close job object -- \
                     edge.exe will not be automatically terminated if the service exits \
                     abnormally: {err}"
                ));
                None
            }
        };
        Self {
            session: Session::Idle,
            generation: 0,
            job,
        }
    }

    /// Kills any running child (ignoring "already exited" as success --
    /// that's the outcome we wanted anyway) and resets to `Idle`.
    fn stop_current(&mut self) {
        if let Some(child) = self.session_child_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.session = Session::Idle;
    }

    fn session_child_mut(&mut self) -> Option<&mut Child> {
        match &mut self.session {
            Session::Starting { child } | Session::Connected { child, .. } => Some(child),
            Session::Idle | Session::Error { .. } => None,
        }
    }

    /// Reaps the child if it has exited on its own since we last looked,
    /// so `Status` reflects reality instead of a stale `Starting`/
    /// `Connected`.
    fn reap_if_exited(&mut self) {
        let exited = match self.session_child_mut() {
            Some(child) => matches!(child.try_wait(), Ok(Some(_))),
            None => false,
        };
        if exited {
            self.session = Session::Error {
                message: "edge.exe exited unexpectedly".to_string(),
            };
        }
    }

    fn status(&mut self) -> ConnectionStatus {
        self.reap_if_exited();
        match &self.session {
            Session::Idle => ConnectionStatus::Idle,
            Session::Starting { .. } => ConnectionStatus::Starting,
            Session::Connected {
                overlay_ip,
                since_unix_secs,
                ..
            } => ConnectionStatus::Connected {
                overlay_ip: overlay_ip.clone(),
                since_unix_secs: *since_unix_secs,
            },
            Session::Error { message } => ConnectionStatus::Error {
                message: message.clone(),
            },
        }
    }
}

impl Default for ServiceState {
    fn default() -> Self {
        Self::new()
    }
}

/// Locks `state`, recovering from mutex poisoning (a prior panic while
/// holding the lock) rather than propagating it -- a single bad request
/// should not permanently wedge the whole service's IPC surface. This
/// pairs with `pipe.rs`'s per-connection `catch_unwind`: if a handler does
/// panic mid-request, we'd rather serve the next request against
/// whatever state was last consistently written than refuse to ever lock
/// the state again.
fn lock_state(state: &Arc<Mutex<ServiceState>>) -> MutexGuard<'_, ServiceState> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Kills the service's currently-running `edge.exe` child, if any. Called
/// from the service control handler on `Stop`/`Shutdown` (see `main.rs`)
/// so a stopped/shut-down service never leaves an orphaned `edge.exe`
/// process behind.
pub fn shutdown(state: &Arc<Mutex<ServiceState>>) {
    lock_state(state).stop_current();
}

/// Logs which `Request` variant arrived. `StartProfile` additionally logs
/// the community/supernode being connected to -- but never the pass key,
/// which is a real secret worth keeping out of a plaintext log file even
/// at this LAN-party-scale threat model.
fn log_request(request: &Request) {
    match request {
        Request::Ping => log("dispatch: received Ping"),
        Request::Status => log("dispatch: received Status"),
        Request::Stop => log("dispatch: received Stop"),
        Request::StartProfile { profile } => log(format!(
            "dispatch: received StartProfile (community={:?}, supernode={:?})",
            profile.community, profile.supernode
        )),
    }
}

pub fn handle(state: &Arc<Mutex<ServiceState>>, request: Request) -> Response {
    log_request(&request);

    let response = match request {
        Request::Ping => Response::Ok,
        Request::Status => Response::Status(lock_state(state).status()),
        Request::Stop => {
            lock_state(state).stop_current();
            Response::Ok
        }
        Request::StartProfile { profile } => start_profile(state, profile),
    };

    log(format!("dispatch: responding {response:?}"));
    response
}

/// Fields needed to call `build_edge_argv`, owned rather than borrowed from
/// a `village_core::profile::ServerProfile` -- there is no such profile
/// here, only the wire-format `ResolvedProfile`, so the validated newtypes
/// are constructed directly from its fields.
struct ValidatedEdgeProfile {
    community: Community,
    key: PassKey,
    supernode: SupernodeAddr,
    mac: MacAddr,
    advanced: AdvancedSettings,
}

fn validate_profile(profile: &ResolvedProfile) -> Result<ValidatedEdgeProfile, String> {
    let community = Community::new(profile.community.clone()).map_err(|err| err.to_string())?;
    let key = PassKey::new(profile.key.clone()).map_err(|err| err.to_string())?;
    let supernode = SupernodeAddr::new(&profile.supernode).map_err(|err| err.to_string())?;
    let mac = parse_mac(&profile.mac)?;
    let cipher = profile.cipher.map(cipher_from_code).transpose()?;
    let compression = profile.compression.map(compression_from_code).transpose()?;

    Ok(ValidatedEdgeProfile {
        community,
        key,
        supernode,
        mac,
        advanced: AdvancedSettings {
            mtu: profile.mtu,
            header_encryption: profile.header_encryption,
            cipher,
            compression,
        },
    })
}

/// Parses a `AA:BB:CC:DD:EE:FF`-style MAC string -- the same format
/// `village_core::mac::MacAddr`'s `Display` impl produces, which is what a
/// well-behaved GUI client sends over the wire. `village_core` does not
/// expose a parser for this (only generation), so it's re-implemented here
/// rather than trusting the string unparsed.
fn parse_mac(value: &str) -> Result<MacAddr, String> {
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() != 6 {
        return Err(format!(
            "mac address must have 6 colon-separated octets, got {}",
            parts.len()
        ));
    }
    let mut bytes = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        if part.len() != 2 {
            return Err(format!(
                "mac address octet {i} must be exactly 2 hex digits, got {part:?}"
            ));
        }
        bytes[i] = u8::from_str_radix(part, 16)
            .map_err(|_| format!("mac address octet {i} ({part:?}) is not valid hex"))?;
    }
    Ok(MacAddr::from_bytes(bytes))
}

/// Mirrors `village_core::profile::Cipher::code()` in reverse.
fn cipher_from_code(code: u8) -> Result<Cipher, String> {
    match code {
        1 => Ok(Cipher::None),
        2 => Ok(Cipher::Twofish),
        3 => Ok(Cipher::Aes),
        4 => Ok(Cipher::ChaCha20),
        5 => Ok(Cipher::Speck),
        other => Err(format!("unknown cipher code {other}")),
    }
}

/// Mirrors `village_core::profile::Compression::code()` in reverse.
fn compression_from_code(code: u8) -> Result<Compression, String> {
    match code {
        1 => Ok(Compression::Lzo1x),
        2 => Ok(Compression::Zstd),
        other => Err(format!("unknown compression code {other}")),
    }
}

/// Returns a copy of `argv` with the value immediately following `-k`
/// (the pass key -- see `build_edge_argv`) replaced with `<redacted>`, so
/// the argv can be logged for visibility without writing the key itself
/// into a plaintext log file.
fn redact_key_arg(argv: &[String]) -> Vec<String> {
    let mut redacted = argv.to_vec();
    if let Some(key_index) = redacted.iter().position(|arg| arg == "-k") {
        if let Some(value) = redacted.get_mut(key_index + 1) {
            *value = "<redacted>".to_string();
        }
    }
    redacted
}

fn start_profile(state: &Arc<Mutex<ServiceState>>, profile: ResolvedProfile) -> Response {
    let validated = match validate_profile(&profile) {
        Ok(validated) => validated,
        Err(message) => {
            return Response::Error {
                code: ErrorCode::InvalidProfile,
                message,
            };
        }
    };

    let edge_args = EdgeArgs {
        community: &validated.community,
        key: &validated.key,
        supernode: &validated.supernode,
        mac: validated.mac,
        advanced: &validated.advanced,
    };
    let argv = build_edge_argv(&edge_args);

    let mut guard = lock_state(state);
    // Only one `edge.exe` session at a time -- replace whatever was
    // running before starting the new one.
    guard.stop_current();
    guard.generation = guard.generation.wrapping_add(1);
    let generation = guard.generation;

    // Fixed path only -- see the module doc comment above. `profile` never
    // influences where this binary comes from.
    let edge_exe_path = install::edge_exe_path();
    log(format!(
        "dispatch: spawning {} {}",
        edge_exe_path.display(),
        redact_key_arg(&argv).join(" ")
    ));

    let mut command = Command::new(&edge_exe_path);
    command
        .args(&argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let message = format!("failed to launch edge.exe: {err}");
            log(format!("dispatch: {message}"));
            guard.session = Session::Error {
                message: message.clone(),
            };
            return Response::Error {
                code: ErrorCode::SpawnFailed,
                message,
            };
        }
    };

    // Assign the freshly-spawned child to the kill-on-close job object (if
    // we managed to create one at startup -- see `ServiceState::new`), so
    // it is terminated automatically if this service process ever exits
    // abnormally instead of surviving as an orphan. Best-effort: a failure
    // here is logged, not fatal -- `edge.exe` is already running and
    // usable, just without this extra hardening.
    if let Some(job) = &guard.job {
        if let Err(err) = job.assign(&child) {
            log(format!(
                "dispatch: failed to assign spawned edge.exe (pid {}) to the kill-on-close \
                 job object -- it may be left running as an orphan if village-service exits \
                 abnormally before it is stopped normally: {err}",
                child.id()
            ));
        }
    }

    let stdout = child.stdout.take();
    guard.session = Session::Starting { child };
    drop(guard);

    match stdout {
        Some(stdout) => spawn_stdout_watcher(Arc::clone(state), generation, stdout),
        None => {
            // Shouldn't happen given `Stdio::piped()` above, but don't
            // leave the session stuck in `Starting` forever if it somehow
            // does.
            mark_error(
                state,
                generation,
                "edge.exe spawned without a capturable stdout handle".to_string(),
            );
        }
    }

    Response::Ok
}

enum WatcherEvent {
    GotIp(String),
    Eof,
    ReadError,
}

/// Spawns a pair of threads that together implement the "wait up to
/// `STARTUP_TIMEOUT` for `edge.exe` to report an overlay IP, else error"
/// behavior:
///
/// - a reader thread blocks on `BufRead::read_line` over `edge.exe`'s
///   stdout (there is no portable non-blocking readline over a child pipe
///   without extra plumbing) and forwards what it finds over a channel;
/// - a supervisor thread applies the actual wall-clock timeout via
///   `Receiver::recv_timeout`, independent of whether the reader thread is
///   currently blocked inside a single `read_line` call.
///
/// If `edge.exe` never prints anything recognizable and is never killed,
/// the reader thread can outlive the timeout (a leaked thread blocked on a
/// dead-end read) -- the `generation` check in `mark_connected`/
/// `mark_error` stops it from clobbering a *newer* session's state if it
/// ever does wake up.
fn spawn_stdout_watcher(state: Arc<Mutex<ServiceState>>, generation: u64, stdout: ChildStdout) {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(WatcherEvent::Eof);
                    return;
                }
                Ok(_) => {
                    // TODO: verify actual edge.exe stdout format against a
                    // real binary -- see `parse_overlay_ip`'s doc comment.
                    if let Some(ip) = parse_overlay_ip(&line) {
                        let _ = tx.send(WatcherEvent::GotIp(ip));
                        return;
                    }
                    // Not a recognizable line yet -- keep reading.
                }
                Err(_) => {
                    let _ = tx.send(WatcherEvent::ReadError);
                    return;
                }
            }
        }
    });

    std::thread::spawn(move || match rx.recv_timeout(STARTUP_TIMEOUT) {
        Ok(WatcherEvent::GotIp(ip)) => mark_connected(&state, generation, ip),
        Ok(WatcherEvent::Eof) => mark_error(
            &state,
            generation,
            "edge.exe closed its output without reporting an IP".to_string(),
        ),
        Ok(WatcherEvent::ReadError) => mark_error(
            &state,
            generation,
            "error reading edge.exe output".to_string(),
        ),
        Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            mark_error(
                &state,
                generation,
                "timed out waiting for edge to report an IP".to_string(),
            )
        }
    });
}

fn mark_connected(state: &Arc<Mutex<ServiceState>>, generation: u64, overlay_ip: String) {
    let mut guard = lock_state(state);
    if guard.generation != generation {
        return; // Superseded by a newer session -- ignore this stale result.
    }
    if matches!(guard.session, Session::Starting { .. }) {
        if let Session::Starting { child } = std::mem::replace(&mut guard.session, Session::Idle) {
            let since_unix_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            guard.session = Session::Connected {
                child,
                overlay_ip,
                since_unix_secs,
            };
        }
    }
    // If the session is no longer `Starting` (e.g. the user hit `Stop`
    // concurrently, which already reset it to `Idle` under this same
    // generation), deliberately do nothing rather than resurrecting a
    // session the user just stopped.
}

fn mark_error(state: &Arc<Mutex<ServiceState>>, generation: u64, message: String) {
    let mut guard = lock_state(state);
    if guard.generation != generation {
        return;
    }
    if matches!(guard.session, Session::Starting { .. }) {
        if let Session::Starting { mut child } =
            std::mem::replace(&mut guard.session, Session::Idle)
        {
            // The child may well still be alive here (e.g. the timeout
            // case) -- kill it so a session we're abandoning doesn't leak
            // an orphaned edge.exe process with nothing left tracking it.
            let _ = child.kill();
            let _ = child.wait();
        }
        guard.session = Session::Error { message };
    }
}

/// Best-effort parse of a single line of `edge.exe` stdout for the overlay
/// IP it was assigned by the supernode.
///
/// # Unverified against real `edge.exe` output
///
/// No Windows host with a working n2n build was available in the session
/// that wrote this. The heuristic below is deliberately conservative: it
/// only fires on a line that both looks IP-related (contains one of a
/// handful of plausible keywords) *and* contains something that parses as
/// an IPv4 address; any other line is simply not treated as a match, and
/// reading continues. Verify this against real `edge.exe` output and
/// adjust the keywords/format before relying on it in production --
/// tracked as a follow-up, not solved here.
fn parse_overlay_ip(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let looks_relevant = lower.contains("ip address")
        || lower.contains("assigned")
        || lower.contains("tap_open")
        || lower.contains("dhcp")
        || lower.contains("ip:");
    if !looks_relevant {
        return None;
    }

    line.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|token| is_ipv4(token))
        .map(|s| s.to_string())
}

fn is_ipv4(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
}
