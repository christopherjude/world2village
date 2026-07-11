//! Named-pipe IPC server. Accepts one client connection at a time (this is
//! a low-traffic control channel, not a data path): for each connection it
//! reads one framed `Request`, dispatches it, writes back one framed
//! `Response`, then serves the next connection.
//!
//! # Pipe access control
//!
//! The pipe's security descriptor is built from the SDDL string in
//! [`PIPE_SDDL`]:
//! - `SY` (SYSTEM) and `BA` (built-in Administrators) get `GA` (generic
//!   all / full control).
//! - `AU` (Authenticated Users) get `GRGW` (generic read + generic write)
//!   only -- enough to open the pipe and exchange framed messages, nothing
//!   more (e.g. no right to change the pipe's own security).
//! - There is deliberately no ACE for `WD` (Everyone) or `AN` (Anonymous
//!   Logon), so unauthenticated/guest connections are refused outright.
//!
//! Combined with `PIPE_REJECT_REMOTE_CLIENTS` (no network clients -- local
//! machine only, not exposed by name over SMB), this keeps the control
//! channel reachable only by processes running as a logged-in user (or
//! admin/SYSTEM) on the same machine.

use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FlushFileBuffers, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use village_ipc::frame::{read_frame, write_frame};
use village_ipc::protocol::{ErrorCode, Request, Response};

use crate::acl::SecurityDescriptor;
use crate::dispatch::{self, ServiceState};

pub const PIPE_NAME: &str = r"\\.\pipe\Village\v1";

/// SYSTEM + Administrators full control, Authenticated Users read/write --
/// see the module doc comment above for the full rationale.
const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)";

/// In/out pipe buffer size. Generous headroom over
/// `village_ipc::frame::MAX_FRAME_LEN` (64 KiB); control messages are small
/// and fixed-shape, this is not a tuning knob.
const BUFFER_SIZE: u32 = 65 * 1024;

/// Runs the accept loop forever. Each connection is handled on its own
/// thread, wrapped in `std::panic::catch_unwind`, so a panic or I/O error
/// handling one client can never take down the whole service process or
/// stop it from serving the next client.
///
/// There is no cooperative shutdown mechanism here: `ConnectNamedPipe` is a
/// blocking synchronous call with no clean cross-thread cancellation short
/// of racily closing the handle from another thread. Instead, the
/// service's control handler (see `main.rs`) stops the whole process
/// directly -- after killing any running `edge.exe` child -- once it
/// receives a `Stop`/`Shutdown` control event, rather than trying to
/// unblock this loop cooperatively.
pub fn run(state: Arc<Mutex<ServiceState>>) -> ! {
    // `FILE_FLAG_FIRST_PIPE_INSTANCE` must only be passed on the very
    // first `CreateNamedPipeW` call for this pipe name -- it's a guard
    // against another process having already squatted the name, and
    // passing it on every subsequent instance-creation call would make
    // *that* call fail instead (since by definition it would no longer be
    // the first instance).
    let mut is_first_instance = true;

    loop {
        match accept_one(is_first_instance) {
            Ok(file) => {
                is_first_instance = false;
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle_client(file, &state);
                    }));
                    if let Err(panic) = result {
                        eprintln!("village-service: client handler panicked: {panic:?}");
                    }
                });
            }
            Err(err) => {
                eprintln!("village-service: failed to create/accept a pipe connection: {err}");
                // Avoid a hot error loop (e.g. if pipe creation is failing
                // repeatedly for some systemic reason) while still
                // retrying rather than giving up on the IPC surface
                // entirely.
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

/// Creates one named-pipe instance, blocks until a client connects to it,
/// and returns it wrapped as a `File` so the caller can use
/// `village_ipc::frame`'s `Read`/`Write`-based framing helpers directly,
/// instead of hand-rolling `ReadFile`/`WriteFile` calls.
fn accept_one(is_first_instance: bool) -> io::Result<File> {
    let sd = SecurityDescriptor::from_sddl(PIPE_SDDL)?;
    let security_attributes = sd.as_security_attributes();

    let pipe_name_wide = to_wide_null(PIPE_NAME);

    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if is_first_instance {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }

    let handle = unsafe {
        CreateNamedPipeW(
            pipe_name_wide.as_ptr(),
            open_mode,
            PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER_SIZE,
            BUFFER_SIZE,
            0,
            &security_attributes,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    let connect_result = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
    if connect_result == 0 {
        let err = io::Error::last_os_error();
        // A client that connects in the brief window between
        // `CreateNamedPipeW` and `ConnectNamedPipe` is reported via this
        // specific error code rather than a `BOOL` success -- that's the
        // documented expected case, not a failure.
        if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
            unsafe {
                CloseHandle(handle);
            }
            return Err(err);
        }
    }

    // SAFETY: `handle` is a valid, freshly-connected named-pipe instance
    // handle that we exclusively own at this point. Wrapping it in a
    // `File` gives it `Read`/`Write`/`Drop` (auto-`CloseHandle`) for free,
    // reusing `village_ipc::frame`'s generic framing helpers instead of
    // duplicating `ReadFile`/`WriteFile` plumbing here.
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}

fn handle_client(mut file: File, state: &Arc<Mutex<ServiceState>>) {
    let payload = match read_frame(&mut file) {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!("village-service: failed to read a request frame: {err}");
            disconnect(&file);
            return;
        }
    };

    let response = match serde_json::from_slice::<Request>(&payload) {
        Ok(request) => dispatch::handle(state, request),
        Err(err) => Response::Error {
            code: ErrorCode::Internal,
            message: format!("malformed request: {err}"),
        },
    };

    let response_bytes = match serde_json::to_vec(&response) {
        Ok(bytes) => bytes,
        Err(err) => {
            // Serializing our own closed `Response` enum should never
            // actually fail; if it somehow does there's nothing more
            // specific to tell the client, so log and drop the connection
            // rather than panicking the handler thread.
            eprintln!("village-service: failed to serialize response: {err}");
            disconnect(&file);
            return;
        }
    };

    match write_frame(&mut file, &response_bytes) {
        Ok(()) => {
            // `DisconnectNamedPipe` below forcibly tears down this pipe
            // instance, which can race ahead of the client's `ReadFile`
            // still draining the bytes we just wrote (surfacing to the
            // client as `ERROR_NO_DATA` / os error 233, "the pipe is being
            // closed"). `FlushFileBuffers` on a named-pipe server handle
            // blocks until the client has actually read all outstanding
            // written data, so calling it here -- after a successful write,
            // before disconnecting -- closes that race. This is the
            // documented Win32 pattern for named-pipe servers, not a
            // workaround.
            flush(&file);
        }
        Err(err) => {
            eprintln!("village-service: failed to write response frame: {err}");
            // Nothing was (fully) written, so there's nothing to flush.
        }
    }

    disconnect(&file);
}

/// Best-effort: blocks until the client has read everything we just wrote,
/// closing the disconnect-before-read race described in `handle_client`.
/// Failure here doesn't change the outcome for the client (which already
/// has its response bytes queued, if not fully read, by this point), so
/// it's logged rather than propagated.
fn flush(file: &File) {
    unsafe {
        if FlushFileBuffers(file.as_raw_handle() as _) == 0 {
            eprintln!(
                "village-service: FlushFileBuffers failed: {}",
                io::Error::last_os_error()
            );
        }
    }
}

/// Best-effort: flushes and disconnects the pipe instance before `File`'s
/// `Drop` closes the handle. Errors here don't change the outcome for the
/// client (which already has its response, if any, by this point), so
/// they're logged rather than propagated.
fn disconnect(file: &File) {
    unsafe {
        if DisconnectNamedPipe(file.as_raw_handle() as _) == 0 {
            eprintln!(
                "village-service: DisconnectNamedPipe failed: {}",
                io::Error::last_os_error()
            );
        }
    }
}

fn to_wide_null(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
