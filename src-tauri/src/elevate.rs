//! Launches the elevated, one-time `village-service.exe install <resource-dir>`
//! step.
//!
//! `village-service.exe`'s `install` subcommand (see
//! `crates/village-service/src/install.rs`) must already be running with
//! Administrator privileges when invoked -- it does not self-elevate. This
//! module is what actually triggers the single UAC prompt a Village user
//! should ever see, via `ShellExecuteW`'s `"runas"` verb. After this
//! succeeds once, the service is registered as `SERVICE_AUTO_START` and the
//! GUI only ever opens the named pipe from then on -- no more elevation
//! prompts.
//!
//! # Unverified
//!
//! Like the rest of this crate, this has not been compiled or run on a real
//! Windows machine in this session -- see the top-level report for what's
//! pending `village-builder`/manual verification.

use std::path::Path;

#[cfg(windows)]
pub fn launch_service_installer(service_exe: &Path, resource_dir: &Path) -> Result<(), String> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb = to_wide_null("runas");
    let file = to_wide_null(&service_exe.to_string_lossy());
    // `resource_dir` is quoted so a path containing spaces (very plausible
    // under `%ProgramFiles%\Village\...`) is passed to `install.rs` as a
    // single argument rather than being split on whitespace.
    let params = to_wide_null(&format!("install \"{}\"", resource_dir.display()));

    // SAFETY: `verb`/`file`/`params` are wide, null-terminated `Vec<u16>`
    // buffers kept alive on this stack frame for the duration of the call,
    // which is synchronous. `hwnd` is null (no owner window) and
    // `lpdirectory` is null (inherit the current directory) -- both
    // explicitly permitted by `ShellExecuteW`'s documented contract.
    let result = unsafe {
        ShellExecuteW(
            0 as HWND,
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL as i32,
        )
    };

    // Per `ShellExecuteW`'s docs, a return value greater than 32 indicates
    // success; anything else is an error code (e.g. the user declining the
    // UAC prompt surfaces as `ERROR_CANCELLED`).
    let result = result as isize;
    if result <= 32 {
        return Err(format!(
            "failed to launch the elevated Village service installer (ShellExecuteW returned {result})"
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn launch_service_installer(_service_exe: &Path, _resource_dir: &Path) -> Result<(), String> {
    // Village v1 is Windows-only (see CLAUDE.md). This stub exists only so
    // the rest of this crate has a chance of being reasoned about/partially
    // type-checked on a non-Windows machine -- it is not a real target and
    // is never expected to be reached in practice.
    Err("service installation is only supported on Windows".to_string())
}

#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
