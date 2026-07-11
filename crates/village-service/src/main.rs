//! `village-service`: the Windows Service that owns the elevated
//! `edge.exe` child process on behalf of the (unelevated) Village GUI.
//!
//! # Security model (read this before touching subprocess/ACL/pipe code)
//!
//! - **Fixed binary path, never from IPC.** The service only ever executes
//!   `%ProgramData%\Village\bin\edge.exe` (see `install::edge_exe_path`).
//!   No field on the wire (`village_ipc::protocol::Request`) can influence
//!   that path. This is the concrete defense against a compromised or
//!   malicious low-privilege IPC client trying to get SYSTEM to execute an
//!   arbitrary binary by redirecting a path.
//! - **Every profile is re-validated server-side.** `StartProfile`'s
//!   `ResolvedProfile` arrives as plain strings/primitives; `dispatch.rs`
//!   re-parses every field through `village_core`'s validated newtype
//!   constructors before building argv. The service never trusts that
//!   data crossing the IPC boundary is already safe just because the GUI
//!   validated it first.
//! - **Closed operation set.** `Request`/`Response`
//!   (`village_ipc::protocol`) are closed enums with
//!   `#[serde(deny_unknown_fields)]` -- there is no free-form command
//!   execution path anywhere on this pipe.
//! - **Pipe ACL'd to SYSTEM + Administrators + Authenticated Users only.**
//!   See `pipe.rs` for the exact SDDL and rationale; no Everyone/Anonymous
//!   access.
//! - **Install-time binaries are locked down too.** `install.rs` copies
//!   `edge.exe` and the tap-windows6 driver files into
//!   `%ProgramData%\Village\bin` and applies an ACL that only
//!   SYSTEM/Administrators can write to, so a standard user cannot swap out
//!   the binaries the SYSTEM-elevated service later executes (or the
//!   driver files it installs from).
//!
//! # Two invocation modes
//!
//! - `village-service.exe install <resource-dir>`: one-time elevated
//!   setup (see `install::run_install`). This binary does **not**
//!   self-elevate -- it must already be running elevated when invoked. The
//!   Tauri app is responsible for launching it via `ShellExecuteW` with
//!   verb `"runas"`, which is what actually triggers the UAC prompt; that
//!   call lives in `src-tauri`, out of scope for this crate.
//! - `village-service.exe run`: the actual service entry point, invoked by
//!   the Windows Service Control Manager once installed. Blocks until the
//!   service is stopped.

mod acl;
mod dispatch;
mod install;
mod job;
mod logging;
mod pipe;

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState as WinServiceState,
    ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use dispatch::ServiceState;

pub(crate) const SERVICE_NAME: &str = "VillageService";
pub(crate) const SERVICE_DISPLAY_NAME: &str = "Village";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("install") => {
            let Some(resource_dir) = args.get(2) else {
                eprintln!("usage: village-service.exe install <resource-dir>");
                return ExitCode::FAILURE;
            };
            match install::run_install(std::path::Path::new(resource_dir)) {
                Ok(()) => {
                    println!("Village service installed and started.");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("install failed: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("run") => {
            logging::log("village-service: run mode entered");
            // Hands off to the Service Control Manager's dispatcher, which
            // blocks this thread until the service is stopped. An error
            // here means the dispatcher itself couldn't start (most
            // commonly: this process wasn't actually launched by the SCM)
            // -- there's no meaningful recovery from that, just report and
            // exit non-zero.
            match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("service dispatcher failed: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: village-service.exe <install <resource-dir>|run>");
            ExitCode::FAILURE
        }
    }
}

define_windows_service!(ffi_service_main, village_service_main);

/// The actual service entry point, called by the system on a background
/// thread once `service_dispatcher::start` has registered us. There is no
/// console/stdout guaranteed to be visible here (services don't have one),
/// so error reporting past this point is limited to the service status
/// itself plus whatever `eprintln!` happens to reach (useful only when
/// running interactively for local testing).
fn village_service_main(_arguments: Vec<OsString>) {
    let state = Arc::new(Mutex::new(ServiceState::new()));

    // `service_control_handler::register` needs the control-handling
    // closure *before* it can hand us the `ServiceStatusHandle` we'd use
    // to report status from within that very closure (e.g. `Stopped`, on
    // `Stop`/`Shutdown`). Break the cycle with a cell the closure reads
    // from and that we fill in immediately after registering succeeds.
    // `ServiceStatusHandle` is `Copy` (just a wrapped raw handle), so this
    // is cheap.
    let status_handle_cell: Arc<Mutex<Option<service_control_handler::ServiceStatusHandle>>> =
        Arc::new(Mutex::new(None));

    let control_state = Arc::clone(&state);
    let control_status_handle_cell = Arc::clone(&status_handle_cell);
    let status_handle = match service_control_handler::register(SERVICE_NAME, move |control| {
        handle_control_event(control, &control_state, &control_status_handle_cell)
    }) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("village-service: failed to register control handler: {err}");
            return;
        }
    };
    *status_handle_cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(status_handle);

    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: WinServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });

    // Runs forever (`-> !`); the only way this service process ends is via
    // `handle_control_event`'s `Stop`/`Shutdown` handling calling
    // `std::process::exit` directly -- see its doc comment for why a
    // cooperative shutdown of the blocking pipe-accept loop isn't used
    // instead.
    pipe::run(state);
}

/// Handles Service Control Manager control codes.
///
/// On `Stop`/`Shutdown`: kills any currently-running `edge.exe` child (so
/// nothing is orphaned), reports `Stopped` to the SCM, and then exits the
/// whole process. Exiting directly -- rather than trying to unwind
/// `pipe::run`'s blocking `ConnectNamedPipe` call cooperatively -- is a
/// deliberate simplification: this service has nothing else to flush or
/// clean up beyond the child process and the final status report, both of
/// which happen before we exit.
fn handle_control_event(
    control: ServiceControl,
    state: &Arc<Mutex<ServiceState>>,
    status_handle_cell: &Arc<Mutex<Option<service_control_handler::ServiceStatusHandle>>>,
) -> ServiceControlHandlerResult {
    match control {
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        ServiceControl::Stop | ServiceControl::Shutdown => {
            logging::log(format!("village-service: received {control:?} control event"));
            dispatch::shutdown(state);

            if let Some(status_handle) = *status_handle_cell.lock().unwrap_or_else(|e| e.into_inner()) {
                let _ = status_handle.set_service_status(ServiceStatus {
                    service_type: SERVICE_TYPE,
                    current_state: WinServiceState::Stopped,
                    controls_accepted: ServiceControlAccept::empty(),
                    exit_code: ServiceExitCode::Win32(0),
                    checkpoint: 0,
                    wait_hint: Duration::default(),
                    process_id: None,
                });
            }

            std::process::exit(0);
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    }
}
