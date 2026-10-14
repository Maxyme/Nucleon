use super::installation::{self, Original};
use frida_gum::NativePointer;
use std::sync::OnceLock;

use log::info;
use std::ffi::{c_char, c_int, c_void};

static ORIG_CHMOD: Original = Original::new();
static ORIG_FCHMOD: Original = Original::new();

/// Interposes chmod to ensure user read and write permissions (0600) are never stripped.
///
/// On macOS, Steam downloading Windows depots can write files with Windows-translated
/// permissions that lack owner read/write (e.g. 0004 or 0000). When Steam subsequently
/// attempts to re-open or append chunks to these files, it fails with errno 13 (EACCES),
/// triggering the dreaded "missing file privileges" Steam dialog.
///
/// # Safety
/// Caller must pass a valid null-terminated C string pointer for `path`.
pub unsafe extern "C" fn hook_chmod(path: *const c_char, mode: libc::mode_t) -> c_int {
    unsafe {
        let Some(pointer) = ORIG_CHMOD.load() else {
            *libc::__error() = libc::EIO;
            return -1;
        };
        let orig: extern "C" fn(*const c_char, libc::mode_t) -> c_int =
            std::mem::transmute(pointer);
        let mut safe_mode = mode | libc::S_IRUSR | libc::S_IWUSR;
        if (mode & (libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH)) != 0 {
            safe_mode |= libc::S_IXUSR;
        }
        orig(path, safe_mode)
    }
}

/// Interposes fchmod to ensure user read and write permissions (0600) are never stripped.
///
/// # Safety
/// Caller must pass a valid file descriptor `fd`.
pub unsafe extern "C" fn hook_fchmod(fd: c_int, mode: libc::mode_t) -> c_int {
    unsafe {
        let Some(pointer) = ORIG_FCHMOD.load() else {
            *libc::__error() = libc::EIO;
            return -1;
        };
        let orig: extern "C" fn(c_int, libc::mode_t) -> c_int = std::mem::transmute(pointer);
        let mut safe_mode = mode | libc::S_IRUSR | libc::S_IWUSR;
        if (mode & (libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH)) != 0 {
            safe_mode |= libc::S_IXUSR;
        }
        orig(fd, safe_mode)
    }
}

static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();

pub fn install_fs_hooks() -> anyhow::Result<()> {
    installation::once(&INSTALLED, || {
        installation::install(&[
            (
                NativePointer(libc::chmod as *mut c_void),
                NativePointer(hook_chmod as *mut c_void),
                &ORIG_CHMOD,
            ),
            (
                NativePointer(libc::fchmod as *mut c_void),
                NativePointer(hook_fchmod as *mut c_void),
                &ORIG_FCHMOD,
            ),
        ])?;
        info!("Installed fs hooks");
        Ok(())
    })
}
