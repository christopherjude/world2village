//! Shared helpers for building Windows security descriptors from SDDL
//! strings and applying them to things this service creates: the named
//! pipe's own security descriptor (see `pipe.rs`) and the DACL applied to
//! `%ProgramData%\Village\bin` and the binaries copied into it at install
//! time (see `install.rs`).
//!
//! SDDL is used (rather than building `EXPLICIT_ACCESS`/`ACL` structures by
//! hand) because it is far less error-prone to write and to review: the
//! access-control intent is legible directly in the string constant at each
//! call site, rather than spread across several `BuildTrusteeWithSidW`/
//! `SetEntriesInAclW` calls.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
};

fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Owns a `PSECURITY_DESCRIPTOR` allocated by
/// `ConvertStringSecurityDescriptorToSecurityDescriptorW`. Frees it with
/// `LocalFree` on drop, per that function's documented contract.
pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    /// Parses `sddl` into a security descriptor.
    ///
    /// The SDDL strings used in this codebase are all `const` literals
    /// reviewed alongside the call site (see `pipe.rs`/`install.rs`), so a
    /// parse failure here indicates a bug in one of those constants, not
    /// untrusted input -- callers should treat it as fatal to whatever
    /// setup step is in progress rather than trying to recover.
    pub fn from_sddl(sddl: &str) -> io::Result<Self> {
        let sddl_wide = to_wide_null(sddl);
        let mut psd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl_wide.as_ptr(),
                SDDL_REVISION_1,
                &mut psd,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(psd))
    }

    /// Builds a `SECURITY_ATTRIBUTES` that points at this descriptor. The
    /// returned value borrows from `self` (via a raw pointer, since the
    /// Win32 struct has no lifetime) and must not be used after `self` is
    /// dropped.
    pub fn as_security_attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }

    /// Extracts the DACL embedded in this security descriptor. The
    /// returned pointer is only valid as long as `self` is alive.
    fn dacl(&self) -> io::Result<*mut ACL> {
        let mut present = 0;
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut defaulted = 0;
        let ok = unsafe {
            GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted)
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if present == 0 || dacl.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SDDL string produced a security descriptor with no DACL present",
            ));
        }
        Ok(dacl)
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0 as _);
            }
        }
    }
}

/// Applies the DACL encoded in `sddl` to the filesystem object (file or
/// directory) at `path`, replacing any existing DACL and marking it
/// protected (`PROTECTED_DACL_SECURITY_INFORMATION`) so `path` does not
/// silently pick up more permissive inherited ACEs from its parent (e.g.
/// from `%ProgramData%` itself, whose default ACL is more permissive than
/// what we want for the binaries the service executes as SYSTEM).
pub fn apply_sddl_to_path(path: &Path, sddl: &str) -> io::Result<()> {
    let sd = SecurityDescriptor::from_sddl(sddl)?;
    let dacl = sd.dacl()?;
    let path_wide = to_wide_null(&path.to_string_lossy());

    // SetNamedSecurityInfoW returns a WIN32_ERROR directly (0 ==
    // ERROR_SUCCESS), not a BOOL -- unlike most of the other calls in this
    // module.
    let result = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl as *const ACL,
            ptr::null(),
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}
