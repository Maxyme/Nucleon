use super::installation::{self, Original};
use frida_gum::NativePointer;
use std::sync::OnceLock;

use log::info;
use std::ffi::{CStr, c_char, c_int, c_void};

static ORIG_POSIX_SPAWN: Original = Original::new();
static ORIG_POSIX_SPAWNP: Original = Original::new();
static ORIG_EXECVE: Original = Original::new();

fn target_keeps_insert(path: &str) -> bool {
    path.ends_with("steam_osx") || path.ends_with("Steam Helper")
}

/// # Safety
/// A non-null envp must reference a null-terminated array of valid C strings,
/// all readable and unchanged until the forwarded spawn/exec call completes.
unsafe fn sanitize_env(envp: *const *const c_char) -> Vec<*const c_char> {
    unsafe {
        let mut clean = Vec::new();
        if envp.is_null() {
            // Even an empty environment must be a null-terminated pointer array;
            // an empty Vec's as_ptr() is not a readable envp[0].
            return vec![std::ptr::null()];
        }

        let mut i = 0;
        while !(*envp.add(i)).is_null() {
            let entry = *envp.add(i);
            let s = CStr::from_ptr(entry).to_string_lossy();
            if !s.starts_with("DYLD_INSERT_LIBRARIES=") {
                clean.push(entry);
            }
            i += 1;
        }
        clean.push(std::ptr::null());
        clean
    }
}

/// Interposes posix_spawn to strip DYLD_INSERT_LIBRARIES from child helper processes.
///
/// # Safety
/// The caller must provide valid pointers matching the POSIX posix_spawn specification.
pub unsafe extern "C" fn hook_posix_spawn(
    pid: *mut libc::pid_t,
    path: *const c_char,
    file_actions: *const libc::posix_spawn_file_actions_t,
    attrp: *const libc::posix_spawnattr_t,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    unsafe {
        let Some(pointer) = ORIG_POSIX_SPAWN.load() else {
            return libc::EIO;
        };
        let orig: extern "C" fn(
            *mut libc::pid_t,
            *const c_char,
            *const libc::posix_spawn_file_actions_t,
            *const libc::posix_spawnattr_t,
            *const *const c_char,
            *const *const c_char,
        ) -> c_int = std::mem::transmute(pointer);

        if path.is_null() || envp.is_null() {
            return orig(pid, path, file_actions, attrp, argv, envp);
        }

        let path_str = CStr::from_ptr(path).to_string_lossy();
        if target_keeps_insert(&path_str) {
            return orig(pid, path, file_actions, attrp, argv, envp);
        }

        let clean = sanitize_env(envp);
        orig(pid, path, file_actions, attrp, argv, clean.as_ptr())
    }
}

/// Interposes posix_spawnp to strip DYLD_INSERT_LIBRARIES from child helper processes.
///
/// # Safety
/// The caller must provide valid pointers matching the POSIX posix_spawnp specification.
pub unsafe extern "C" fn hook_posix_spawnp(
    pid: *mut libc::pid_t,
    path: *const c_char,
    file_actions: *const libc::posix_spawn_file_actions_t,
    attrp: *const libc::posix_spawnattr_t,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    unsafe {
        let Some(pointer) = ORIG_POSIX_SPAWNP.load() else {
            return libc::EIO;
        };
        let orig: extern "C" fn(
            *mut libc::pid_t,
            *const c_char,
            *const libc::posix_spawn_file_actions_t,
            *const libc::posix_spawnattr_t,
            *const *const c_char,
            *const *const c_char,
        ) -> c_int = std::mem::transmute(pointer);

        if path.is_null() || envp.is_null() {
            return orig(pid, path, file_actions, attrp, argv, envp);
        }

        let path_str = CStr::from_ptr(path).to_string_lossy();
        if target_keeps_insert(&path_str) {
            return orig(pid, path, file_actions, attrp, argv, envp);
        }

        let clean = sanitize_env(envp);
        orig(pid, path, file_actions, attrp, argv, clean.as_ptr())
    }
}

/// Interposes execve to sanitize environment variables.
///
/// # Safety
/// The caller must provide valid pointers matching the POSIX execve specification.
pub unsafe extern "C" fn hook_execve(
    path: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    unsafe {
        let Some(pointer) = ORIG_EXECVE.load() else {
            *libc::__error() = libc::EIO;
            return -1;
        };
        let orig: extern "C" fn(
            *const c_char,
            *const *const c_char,
            *const *const c_char,
        ) -> c_int = std::mem::transmute(pointer);

        if path.is_null() || envp.is_null() {
            return orig(path, argv, envp);
        }

        let path_str = CStr::from_ptr(path).to_string_lossy();
        if target_keeps_insert(&path_str) {
            return orig(path, argv, envp);
        }

        let clean = sanitize_env(envp);
        orig(path, argv, clean.as_ptr())
    }
}

static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();

pub fn install_spawn_hooks() -> anyhow::Result<()> {
    installation::once(&INSTALLED, || {
        installation::install(&[
            (
                NativePointer(libc::posix_spawn as *mut c_void),
                NativePointer(hook_posix_spawn as *mut c_void),
                &ORIG_POSIX_SPAWN,
            ),
            (
                NativePointer(libc::posix_spawnp as *mut c_void),
                NativePointer(hook_posix_spawnp as *mut c_void),
                &ORIG_POSIX_SPAWNP,
            ),
            (
                NativePointer(libc::execve as *mut c_void),
                NativePointer(hook_execve as *mut c_void),
                &ORIG_EXECVE,
            ),
        ])?;
        info!("Installed spawn hooks");
        Ok(())
    })
}
