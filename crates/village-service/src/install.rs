//! One-time elevated setup: copies `edge.exe` and the tap-windows6 driver
//! files into `%ProgramData%\Village\bin`, locks that directory (and the
//! files in it) down with an ACL that a non-admin user cannot write to,
//! (re)installs the tap-windows6 virtual adapter -- always removing any
//! existing one first, so a re-run always ends up with the driver files
//! that were just copied, not a possibly-stale prior version -- and
//! registers + starts the Windows Service so it is running before this
//! process exits (no reboot, no manual `net start`, required before first
//! use).
//!
//! # Why tap-windows6, not WinTun
//!
//! See `CLAUDE.md`'s gotchas: WinTun is a layer-3 (IP-only) adapter and n2n
//! is a layer-2 (Ethernet) system, so WinTun does not work with n2n. Village
//! installs the real tap-windows6 driver (OpenVPN's) instead, automated
//! here so the end user never sees a driver-install dialog.
//!
//! # Assumption: this process is already elevated
//!
//! `run_install` does **not** self-elevate. It must already be running
//! with Administrator privileges when called. The Tauri app is responsible
//! for launching `village-service.exe install <resource-dir>` via
//! `ShellExecuteW` with the `"runas"` verb, which is what actually triggers
//! the UAC prompt -- that call lives in `src-tauri`, out of scope for this
//! crate. If this process is not actually elevated, the ACL, driver-install,
//! and/or service registration calls below will simply fail with an
//! access-denied error, which is surfaced to the caller rather than
//! silently ignored.
//!
//! # Scope
//!
//! Re-running `install` is expected and supported (e.g. after the service
//! process was killed/crashed, or across an app reinstall/update) -- not
//! just a fresh-install path. The file copies overwrite in place, the ACLs
//! are reapplied unconditionally, and the tap-windows6 driver install step
//! is re-run-safe: it always removes any existing `tap0901` adapter before
//! reinstalling (see `install_tap_driver`), so a repeat run never leaves a
//! stale driver behind or creates a duplicate/orphaned virtual adapter.
//! Service registration (`register_and_start_service`) is likewise
//! idempotent: an SCM service registration persists across the service
//! process dying and across app reinstall (it isn't tied to our
//! install/uninstall flow), so a repeat run falls back to reconfiguring and
//! (re)starting the already-registered service instead of failing with
//! `ERROR_SERVICE_EXISTS`.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::acl;
use crate::logging::log;
use crate::{SERVICE_DISPLAY_NAME, SERVICE_NAME};

/// ACL applied to `%ProgramData%\Village\bin` itself: SYSTEM (`SY`) and
/// built-in Administrators (`BA`) get `FA` (file all access / full
/// control), inherited by anything created under it (`OICI` = object
/// inherit + container inherit); Authenticated Users (`AU`) get `GRGX`
/// (generic read + generic execute) only -- enough to run `edge.exe` via
/// the service and to have the directory be listable/traversable, but not
/// to write, rename, or delete anything in it. The leading `P` marks the
/// DACL protected so it does not also inherit `%ProgramData%`'s own
/// (more permissive) default ACL.
const DIR_ACL_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;AU)";

/// Same intent as `DIR_ACL_SDDL`, applied directly to each copied file
/// (directory ACL inheritance only reliably covers files created *after*
/// the directory's ACL is set; re-applying explicitly to the files we just
/// copied is belt-and-suspenders against relying on inheritance timing).
const FILE_ACL_SDDL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;AU)";

const BIN_FILES: &[&str] = &["edge.exe"];

/// The tap-windows6 driver files, sourced from the `amd64/` folder of an
/// OpenVPN/tap-windows6 release's `dist.win10.zip` (see `bin/README.md`).
/// Copied from `<resource-dir>/tap-driver/` into
/// `%ProgramData%\Village\bin\tap-driver\`.
const DRIVER_FILES: &[&str] = &["devcon.exe", "OemVista.inf", "tap0901.cat", "tap0901.sys"];

/// Hardware ID used by both the presence check (`devcon find`) and the
/// install command (`devcon install OemVista.inf tap0901`).
const TAP_HARDWARE_ID: &str = "tap0901";

/// Errors from the install flow. Deliberately a thin wrapper rather than a
/// rich error hierarchy -- this is a one-shot CLI path whose only consumer
/// is a human running the installer (or the Tauri app surfacing the
/// message in a dialog), not a machine-parsed API.
#[derive(Debug)]
pub enum InstallError {
    Io(io::Error),
    Service(windows_service::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::Io(err) => write!(f, "{err}"),
            InstallError::Service(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InstallError::Io(err) => Some(err),
            InstallError::Service(err) => Some(err),
        }
    }
}

impl From<io::Error> for InstallError {
    fn from(err: io::Error) -> Self {
        InstallError::Io(err)
    }
}

impl From<windows_service::Error> for InstallError {
    fn from(err: windows_service::Error) -> Self {
        InstallError::Service(err)
    }
}

pub fn run_install(resource_dir: &Path) -> Result<(), InstallError> {
    log(format!(
        "install: starting, resource_dir={}",
        resource_dir.display()
    ));

    let bin_dir = install_bin_dir();
    std::fs::create_dir_all(&bin_dir)?;

    for file_name in BIN_FILES {
        let src = resource_dir.join(file_name);
        let dst = bin_dir.join(file_name);
        copy_with_context(&src, &dst)?;
    }

    let driver_dir = tap_driver_dir();
    std::fs::create_dir_all(&driver_dir)?;
    let driver_src_dir = resource_dir.join("tap-driver");
    for file_name in DRIVER_FILES {
        let src = driver_src_dir.join(file_name);
        let dst = driver_dir.join(file_name);
        copy_with_context(&src, &dst)?;
    }

    // Lock the directory down first (so anything dropped into it in the
    // future -- including the tap-driver subdirectory -- inherits the
    // restrictive ACL by default), then re-apply explicitly to the files we
    // just copied (directory ACL inheritance only reliably covers files
    // created *after* the directory's ACL is set).
    acl::apply_sddl_to_path(&bin_dir, DIR_ACL_SDDL)?;
    for file_name in BIN_FILES {
        acl::apply_sddl_to_path(&bin_dir.join(file_name), FILE_ACL_SDDL)?;
    }
    acl::apply_sddl_to_path(&driver_dir, DIR_ACL_SDDL)?;
    for file_name in DRIVER_FILES {
        acl::apply_sddl_to_path(&driver_dir.join(file_name), FILE_ACL_SDDL)?;
    }

    // The driver must be ready before the service is ever started against
    // it (the service immediately runs `edge.exe`, which needs a working
    // tap-windows6 adapter to open).
    install_tap_driver(&driver_dir)?;

    match register_and_start_service() {
        Ok(()) => log("install: service registered and started"),
        Err(err) => {
            log(format!("install: service registration failed: {err}"));
            return Err(err.into());
        }
    }

    log("install: complete");
    Ok(())
}

/// Copies `src` to `dst`, wrapping any failure with enough context (both
/// paths, plus the original error) to be actionable when surfaced to a
/// user via whatever error-reporting path the caller uses -- matches the
/// pattern already used for `edge.exe`/driver file copies. Logs both the
/// success and failure case so a failed install (e.g. the resource-path
/// mismatch that originally prompted this logging) leaves a trail of
/// exactly which file, if any, didn't make it.
fn copy_with_context(src: &Path, dst: &Path) -> io::Result<()> {
    match std::fs::copy(src, dst) {
        Ok(_) => {
            log(format!("install: copied {} to {}", src.display(), dst.display()));
            Ok(())
        }
        Err(err) => {
            log(format!(
                "install: failed to copy {} to {}: {err}",
                src.display(),
                dst.display()
            ));
            Err(io::Error::new(
                err.kind(),
                format!("failed to copy {} to {}: {err}", src.display(), dst.display()),
            ))
        }
    }
}

/// `%ProgramData%\Village\bin` -- the fixed, service-owned directory where
/// `edge.exe` and the tap-driver subdirectory live.
fn install_bin_dir() -> PathBuf {
    program_data_dir().join("Village").join("bin")
}

/// `%ProgramData%\Village\bin\tap-driver` -- where the tap-windows6 driver
/// files (`devcon.exe`, `OemVista.inf`, `tap0901.cat`, `tap0901.sys`) are
/// copied to and installed from.
fn tap_driver_dir() -> PathBuf {
    install_bin_dir().join("tap-driver")
}

/// Checks whether the tap-windows6 adapter is already present, using
/// `devcon.exe find tap0901`. Diagnostic/log-only -- see `install_tap_driver`
/// for why its result no longer gates whether the install step runs.
///
/// # Why `find` and not `hwids`
///
/// Both `devcon find <id>` and `devcon hwids <id>` can report presence, but
/// `find` is the simpler of the two to interpret here: it matches against
/// already-installed devices by hardware ID and its exit status directly
/// reflects whether it found a match (`0` = at least one match, non-zero =
/// none/error), so no stdout text-scraping is needed. `hwids` instead lists
/// hardware IDs for devices selected by a device *class* or pattern and is
/// meant for discovering IDs, not confirming one is already installed, so
/// `find` is the more direct fit.
fn is_driver_installed(driver_dir: &Path) -> io::Result<bool> {
    let devcon = driver_dir.join("devcon.exe");
    let status = std::process::Command::new(&devcon)
        .current_dir(driver_dir)
        .args(["find", TAP_HARDWARE_ID])
        .status()
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to run {} find {TAP_HARDWARE_ID}: {err}", devcon.display()),
            )
        })?;
    Ok(status.success())
}

/// Runs `devcon.exe` with `args`, logging the exact command invoked plus
/// its exit code and captured stdout/stderr -- this is the diagnostic
/// trail the driver install step lacked when it silently failed during
/// live testing.
fn run_devcon_logged(devcon: &Path, driver_dir: &Path, args: &[&str]) -> io::Result<std::process::Output> {
    log(format!("install: running {} {}", devcon.display(), args.join(" ")));
    let output = std::process::Command::new(devcon)
        .current_dir(driver_dir)
        .args(args)
        .output()
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to run {} {}: {err}", devcon.display(), args.join(" ")),
            )
        })?;
    log(format!(
        "install: {} {} exited with {:?}, stdout={:?}, stderr={:?}",
        devcon.display(),
        args.join(" "),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ));
    Ok(output)
}

/// Unconditionally reinstalls the tap-windows6 virtual adapter: remove
/// whatever is currently there, then install fresh from the driver files we
/// just copied.
///
/// # Why unconditional, not "skip if already present"
///
/// This matches the pattern used by the prior-art n2n Windows wrapper
/// happynclient/happynwindows (referenced for UX/packaging ideas per
/// `CLAUDE.md`, not code reuse): their NSIS installer always runs
/// `tapinstall find` (log only), then unconditionally
/// `tapinstall remove TAP0901` followed by
/// `tapinstall install OemVista.inf TAP0901` on every install run. A
/// presence check alone can't tell a healthy, current-version adapter from a
/// stale one left behind by a previous app version's driver files -- across
/// an app update, `OemVista.inf`/`tap0901.sys` in `driver_dir` may have
/// changed, but a "skip if found" check would leave the old driver in place
/// since the hardware ID doesn't change between versions. Always
/// remove-then-install guarantees whatever's running matches the files this
/// install run just laid down, at the cost of a couple of quick `devcon`
/// calls on every run.
///
/// `remove`'s exit code is intentionally not treated as fatal: it's expected
/// to "fail" harmlessly (non-zero) when the adapter wasn't present yet (e.g.
/// first-ever install), and we only care that the adapter is gone before we
/// (re)install it, not why. `install`'s exit code does matter and is
/// propagated as a real error, same as before.
fn install_tap_driver(driver_dir: &Path) -> Result<(), InstallError> {
    let devcon = driver_dir.join("devcon.exe");

    let was_present = is_driver_installed(driver_dir)?;
    log(format!(
        "install: tap-windows6 adapter ({TAP_HARDWARE_ID}) present before reinstall: {was_present}"
    ));

    let remove_output = run_devcon_logged(&devcon, driver_dir, &["remove", TAP_HARDWARE_ID])?;
    if !remove_output.status.success() {
        log(format!(
            "install: devcon remove {TAP_HARDWARE_ID} exited with {:?} (expected/harmless if the adapter wasn't already installed)",
            remove_output.status.code()
        ));
    }

    let install_output =
        run_devcon_logged(&devcon, driver_dir, &["install", "OemVista.inf", TAP_HARDWARE_ID])?;

    if !install_output.status.success() {
        let message = format!(
            "devcon install OemVista.inf {TAP_HARDWARE_ID} failed with exit code {:?}",
            install_output.status.code()
        );
        log(format!("install: {message}"));
        return Err(InstallError::Io(io::Error::new(io::ErrorKind::Other, message)));
    }

    Ok(())
}

pub(crate) fn program_data_dir() -> PathBuf {
    // `%ProgramData%` is set by Windows itself for every process; the
    // fallback below only matters for the (untested-on-real-Windows)
    // theoretical case where it's been unset, so a plausible default is
    // enough -- this is not a security boundary, just a location.
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

/// The fixed path this service always executes `edge.exe` from. Kept here
/// as the single source of truth (also used by `dispatch.rs`) so the
/// "never trust a path from the IPC caller" guarantee has exactly one
/// place that decides what the real path is.
pub fn edge_exe_path() -> PathBuf {
    install_bin_dir().join("edge.exe")
}

/// Win32 error code for `ERROR_SERVICE_EXISTS`: returned (wrapped as
/// `windows_service::Error::Winapi`) by `create_service` when a service
/// with this name is already registered with the SCM. This happens on
/// every "Setup Village" run after the first successful one, because an
/// SCM service registration is persistent -- it outlives the service
/// process being killed and even an app reinstall (SCM registrations
/// aren't tied to our NSIS/MSI install/uninstall) -- so re-running install
/// is a normal, expected scenario, not an edge case.
const ERROR_SERVICE_EXISTS: i32 = 1073;

/// Win32 error code for `ERROR_SERVICE_ALREADY_RUNNING`: returned (wrapped
/// the same way) by `Service::start` when the service is already up. That
/// is exactly the desired end state here, so it's treated as success
/// rather than propagated.
const ERROR_SERVICE_ALREADY_RUNNING: i32 = 1056;

/// Returns the underlying Win32 error code for a `windows_service::Error`,
/// if it is the `Winapi` variant wrapping an OS error. Every fallible call
/// in this module's crate version (`windows-service` 0.8.1) that can fail
/// with an OS-level error constructs `Error::Winapi(io::Error::last_os_error())`
/// (see `create_service`/`open_service`/`Service::start`/`Service::change_config`
/// in the crate source), so `io::Error::raw_os_error()` is the correct way
/// to recover the specific Win32 code -- there is no more targeted
/// error-code accessor exposed by this crate version.
fn win32_error_code(err: &windows_service::Error) -> Option<i32> {
    match err {
        windows_service::Error::Winapi(io_err) => io_err.raw_os_error(),
        _ => None,
    }
}

/// Registers `VillageService` with the SCM and starts it, idempotently.
///
/// # Why this needs a fallback path
///
/// An SCM service registration is persistent: it survives the service
/// process being killed (crash, Task Manager, etc.) and even survives the
/// app being reinstalled, since SCM registrations aren't tied to our
/// install/uninstall flow. So the very first "Setup Village" run
/// registers the service fresh via `create_service`, but every subsequent
/// run (after a crash, a reinstall, or just re-running setup) hits
/// `ERROR_SERVICE_EXISTS` on that same call. That is a completely normal,
/// expected scenario -- not a real failure -- so it's handled explicitly
/// here: fall back to opening the already-registered service, refresh its
/// config (in case the app's install location or binary changed since it
/// was first registered) and description, then start it, treating
/// "already running" as success too. Any other error from any of these
/// calls is still propagated as a real failure.
fn register_and_start_service() -> windows_service::Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)?;

    let service_binary_path = std::env::current_exe().map_err(windows_service::Error::Winapi)?;

    let service_info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: service_binary_path,
        launch_arguments: vec![OsString::from("run")],
        dependencies: vec![],
        // `None` runs the service as LocalSystem -- required so it can set
        // the tap-windows6 adapter's IP (see CLAUDE.md's elevation gotcha).
        account_name: None,
        account_password: None,
    };

    let service_access = ServiceAccess::CHANGE_CONFIG | ServiceAccess::START;

    let service = match service_manager.create_service(&service_info, service_access) {
        Ok(service) => {
            log("install: service created fresh");
            service
        }
        Err(err) if win32_error_code(&err) == Some(ERROR_SERVICE_EXISTS) => {
            log("install: service already registered, reconfiguring and (re)starting");
            let service = service_manager.open_service(SERVICE_NAME, service_access)?;
            service.change_config(&service_info)?;
            service
        }
        Err(err) => return Err(err),
    };

    service.set_description("Owns the elevated edge.exe process for Village.")?;

    match service.start::<&OsStr>(&[]) {
        Ok(()) => Ok(()),
        Err(err) if win32_error_code(&err) == Some(ERROR_SERVICE_ALREADY_RUNNING) => {
            log("install: service already running");
            Ok(())
        }
        Err(err) => Err(err),
    }
}
