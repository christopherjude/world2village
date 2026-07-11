//! Thin client for `village-service`'s named-pipe control channel.
//!
//! Opens a fresh pipe handle per request (see `state.rs`'s doc comment for
//! why no persistent connection is kept), writes one framed `Request`, and
//! reads back exactly one framed `Response` -- matching `village-service`'s
//! `pipe.rs`, which handles one request per connection then disconnects.

use std::fs::OpenOptions;
use std::io;
use std::time::Duration;

use village_ipc::frame::{read_frame, write_frame};
use village_ipc::protocol::{Request, Response};

/// Extra attempts for a transient connection-stage I/O failure (see
/// `send_request`'s doc comment) -- deliberately small and fixed, this is a
/// low-traffic control channel, not a case that needs configurable retry
/// counts or backoff.
const MAX_RETRIES: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_millis(75);

/// Must match `village-service`'s `pipe::PIPE_NAME` exactly.
pub const PIPE_NAME: &str = r"\\.\pipe\Village\v1";

/// Errors from a single request/response round trip over the pipe.
#[derive(Debug)]
pub enum IpcClientError {
    /// The pipe doesn't exist -- i.e. `village-service` is not installed
    /// (or not running). Callers use this to distinguish "you need to run
    /// one-time setup" from a generic failure.
    NotInstalled,
    /// Any other I/O failure opening, writing to, or reading from the pipe.
    Io(io::Error),
    /// The request couldn't be serialized, or the response couldn't be
    /// deserialized/parsed as valid JSON.
    Serialize(serde_json::Error),
}

impl std::fmt::Display for IpcClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcClientError::NotInstalled => {
                write!(f, "the Village service is not installed or not running")
            }
            IpcClientError::Io(err) => write!(f, "IPC I/O error: {err}"),
            IpcClientError::Serialize(err) => write!(f, "IPC message parse error: {err}"),
        }
    }
}

impl std::error::Error for IpcClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IpcClientError::NotInstalled => None,
            IpcClientError::Io(err) => Some(err),
            IpcClientError::Serialize(err) => Some(err),
        }
    }
}

/// Sends `req` to `village-service` and returns its `Response`.
///
/// Opens `\\.\pipe\Village\v1` with plain `std::fs::OpenOptions` -- on
/// Windows, named pipes are addressable as regular files via this path, so
/// std's ordinary file I/O (and `village_ipc::frame`'s generic
/// `Read`/`Write`-based framing helpers) work without any extra pipe-
/// specific API surface on the client side.
///
/// Transparently retries up to [`MAX_RETRIES`] extra times if a single
/// round trip fails with what looks like a transient pipe-teardown race
/// (e.g. the server disconnecting its end just as we're opening/writing/
/// reading) -- see `village-service`'s `pipe.rs` for the server-side fix
/// this is defense-in-depth for. A pipe that doesn't exist at all
/// (`NotInstalled`) is never retried: that's a definitive "service isn't
/// installed/running" state, not a transient blip, and retrying can't fix
/// it.
pub fn send_request(req: &Request) -> Result<Response, IpcClientError> {
    let mut last_err = None;

    for attempt in 0..=MAX_RETRIES {
        match try_send_request(req) {
            Ok(response) => return Ok(response),
            Err(err) => {
                if attempt < MAX_RETRIES && is_transient(&err) {
                    last_err = Some(err);
                    std::thread::sleep(RETRY_DELAY);
                    continue;
                }
                return Err(err);
            }
        }
    }

    // Unreachable in practice: the loop above always returns on its last
    // iteration (either `Ok`, or `Err` because `attempt < MAX_RETRIES` is
    // false). `last_err` is guaranteed `Some` by the time we'd get here.
    Err(last_err.expect("loop always returns before exhausting without an error"))
}

/// One request/response round trip, with no retry logic -- see
/// `send_request`, which wraps this with the retry behavior.
fn try_send_request(req: &Request) -> Result<Response, IpcClientError> {
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME)
        .map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                IpcClientError::NotInstalled
            } else {
                IpcClientError::Io(err)
            }
        })?;

    let payload = serde_json::to_vec(req).map_err(IpcClientError::Serialize)?;
    write_frame(&mut pipe, &payload).map_err(IpcClientError::Io)?;

    let response_bytes = read_frame(&mut pipe).map_err(IpcClientError::Io)?;
    let response: Response =
        serde_json::from_slice(&response_bytes).map_err(IpcClientError::Serialize)?;

    Ok(response)
}

/// Whether `err` looks like a transient pipe-busy/no-data/broken-pipe
/// condition worth retrying, as opposed to a definitive failure.
/// `NotInstalled` (pipe doesn't exist) and `Serialize` errors are never
/// transient -- retrying either can't help.
fn is_transient(err: &IpcClientError) -> bool {
    let IpcClientError::Io(io_err) = err else {
        return false;
    };

    // Known Win32 error codes for pipe-teardown races: ERROR_BROKEN_PIPE
    // (109), ERROR_PIPE_BUSY (231, another client mid-connect), ERROR_NO_DATA
    // (232, "the pipe is being closed"), ERROR_PIPE_NOT_CONNECTED (233, "no
    // process is on the other end of the pipe" -- the symptom this whole fix
    // is for).
    matches!(io_err.raw_os_error(), Some(109 | 231 | 232 | 233))
        || matches!(
            io_err.kind(),
            io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
        )
}
