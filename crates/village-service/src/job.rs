//! Ties the spawned `edge.exe` child's lifetime to this service process's
//! own lifetime, via a Windows Job Object.
//!
//! # Why this exists
//!
//! Before this module existed, `edge.exe` was spawned as a plain child
//! process with nothing else linking its lifetime to `village-service.exe`'s
//! own. If the service process ever ended abnormally -- killed outright
//! (e.g. via Task Manager, which is how this was first observed), crashed,
//! or had its binary overwritten mid-run by an update -- `edge.exe` kept
//! running as an orphaned SYSTEM process with no owner. Concretely, that
//! orphan then held an open handle on
//! `%ProgramData%\Village\bin\edge.exe`, which made every subsequent
//! install/update attempt fail silently at the first file-copy step with
//! `ERROR_SHARING_VIOLATION`.
//!
//! A Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is the standard
//! Win32 mechanism for "child process must die when the parent does, no
//! matter how the parent exits": Windows terminates every process still
//! assigned to the job the moment the job's last handle closes, which
//! happens automatically when `village-service.exe` exits for any reason,
//! since the job handle lives in this process and nowhere else.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// Owns a single anonymous Job Object handle, configured to kill every
/// process assigned to it as soon as this handle is closed. One instance is
/// created at service startup (see `dispatch::ServiceState::new`) and
/// reused across `StartProfile`/`Stop` cycles for the life of the service
/// process -- there is no need for more than one job object, since the
/// service only ever runs a single `edge.exe` child at a time.
pub struct EdgeJob(HANDLE);

// SAFETY: a Win32 `HANDLE` is just an opaque kernel object reference; the
// underlying kernel object is not tied to a particular OS thread, so moving
// or sharing (behind `&EdgeJob`, which is all `assign` needs) this handle
// across threads is sound. `ServiceState` already requires this to be
// `Send`/`Sync` to live behind `Arc<Mutex<_>>`.
unsafe impl Send for EdgeJob {}
unsafe impl Sync for EdgeJob {}

impl EdgeJob {
    /// Creates a new anonymous (unnamed) job object with
    /// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` set, so every process later
    /// assigned to it is terminated the moment this handle is closed
    /// (including implicitly, by this process exiting for any reason).
    pub fn create() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(handle);

        // `Default` zero-initializes every field -- every limit other than
        // the one we set below (working-set limits, affinity, memory
        // limits, I/O counters, ...) is left at its zero/"unset" value,
        // which is exactly what we want: no limits beyond kill-on-close.
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            // `job`'s `Drop` still runs and closes the handle -- an
            // un-configured job object left assigned to nothing is
            // harmless, but there's no point keeping it around, so just
            // propagate the error and let it drop.
            return Err(err);
        }

        Ok(job)
    }

    /// Assigns `child` to this job, so it will be killed if this job's
    /// handle (i.e. this whole service process) ever goes away.
    ///
    /// # Race caveat
    ///
    /// There is a theoretical window between the caller's `Command::spawn`
    /// returning and this call running in which `child` could already have
    /// exited (e.g. `AssignProcessToJobObject` would then fail). For a
    /// long-lived network daemon like `edge.exe`, which does not exit
    /// within microseconds under normal operation, this is not a practical
    /// concern -- and the alternative (spawning suspended and resuming
    /// only after assignment) is more machinery than this problem
    /// warrants. If assignment does fail, the caller logs it and continues
    /// without hardening this one child's lifetime, rather than treating
    /// it as fatal to starting `edge.exe` at all.
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        let process_handle = child.as_raw_handle() as HANDLE;
        let ok = unsafe { AssignProcessToJobObject(self.0, process_handle) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for EdgeJob {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
