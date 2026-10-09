use crate::get_gum;
use frida_gum::interceptor::Interceptor;
use frida_gum::NativePointer;
use log::info;
use std::ffi::{c_char, c_int, c_void};

pub static mut ORIG_CHMOD: *mut c_void = std::ptr::null_mut();
pub static mut ORIG_FCHMOD: *mut c_void = std::ptr::null_mut();

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
    let orig: extern "C" fn(*const c_char, libc::mode_t) -> c_int = std::mem::transmute(ORIG_CHMOD);
    let mut safe_mode = mode | libc::S_IRUSR | libc::S_IWUSR;
    if (mode & (libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH)) != 0 {
        safe_mode |= libc::S_IXUSR;
    }
    orig(path, safe_mode)
}

/// Interposes fchmod to ensure user read and write permissions (0600) are never stripped.
///
/// # Safety
/// Caller must pass a valid file descriptor `fd`.
pub unsafe extern "C" fn hook_fchmod(fd: c_int, mode: libc::mode_t) -> c_int {
    let orig: extern "C" fn(c_int, libc::mode_t) -> c_int = std::mem::transmute(ORIG_FCHMOD);
    let mut safe_mode = mode | libc::S_IRUSR | libc::S_IWUSR;
    if (mode & (libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH)) != 0 {
        safe_mode |= libc::S_IXUSR;
    }
    orig(fd, safe_mode)
}

pub fn install_fs_hooks() -> Result<(), anyhow::Error> {
    let gum = get_gum();
    let mut interceptor = Interceptor::obtain(gum);

    unsafe {
        let chmod_ptr = libc::chmod as *mut c_void;
        let orig_chmod = interceptor
            .replace_fast(
                NativePointer(chmod_ptr),
                NativePointer(hook_chmod as *mut c_void),
            )
            .map_err(|e| anyhow::anyhow!("Frida Gum hook chmod failed: {e:?}"))?;
        ORIG_CHMOD = orig_chmod.0;

        let fchmod_ptr = libc::fchmod as *mut c_void;
        let orig_fchmod = interceptor
            .replace_fast(
                NativePointer(fchmod_ptr),
                NativePointer(hook_fchmod as *mut c_void),
            )
            .map_err(|e| anyhow::anyhow!("Frida Gum hook fchmod failed: {e:?}"))?;
        ORIG_FCHMOD = orig_fchmod.0;

        info!("Installed filesystem permission guards (chmod, fchmod) to prevent 'missing file privileges'");
    }
    Ok(())
}
