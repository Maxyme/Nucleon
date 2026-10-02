use std::ffi::{CStr, c_char, c_int, c_void};
use frida_gum::interceptor::Interceptor;
use frida_gum::{Gum, NativePointer};
use log::info;

pub static mut ORIG_POSIX_SPAWN: *mut c_void = std::ptr::null_mut();
pub static mut ORIG_EXECVE: *mut c_void = std::ptr::null_mut();

fn target_keeps_insert(path: &str) -> bool {
    path.ends_with("steam_osx")
}

unsafe fn sanitize_env(envp: *const *const c_char) -> Vec<*const c_char> {
    let mut clean = Vec::new();
    if envp.is_null() {
        return clean;
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
    let orig: extern "C" fn(
        *mut libc::pid_t,
        *const c_char,
        *const libc::posix_spawn_file_actions_t,
        *const libc::posix_spawnattr_t,
        *const *const c_char,
        *const *const c_char,
    ) -> c_int = std::mem::transmute(ORIG_POSIX_SPAWN);

    if path.is_null() {
        return orig(pid, path, file_actions, attrp, argv, envp);
    }

    let path_str = CStr::from_ptr(path).to_string_lossy();
    if target_keeps_insert(&path_str) {
        return orig(pid, path, file_actions, attrp, argv, envp);
    }

    let clean = sanitize_env(envp);
    orig(pid, path, file_actions, attrp, argv, clean.as_ptr())
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
    let orig: extern "C" fn(
        *const c_char,
        *const *const c_char,
        *const *const c_char,
    ) -> c_int = std::mem::transmute(ORIG_EXECVE);

    if path.is_null() {
        return orig(path, argv, envp);
    }

    let path_str = CStr::from_ptr(path).to_string_lossy();
    if target_keeps_insert(&path_str) {
        return orig(path, argv, envp);
    }

    let clean = sanitize_env(envp);
    orig(path, argv, clean.as_ptr())
}

pub fn install_spawn_hooks() -> Result<(), anyhow::Error> {
    let gum = Gum::obtain();
    let mut interceptor = Interceptor::obtain(&gum);

    unsafe {
        let spawn_ptr = libc::posix_spawn as *mut c_void;
        let execve_ptr = libc::execve as *mut c_void;

        let orig_spawn = interceptor
            .replace_fast(
                NativePointer(spawn_ptr),
                NativePointer(hook_posix_spawn as *mut c_void),
            )
            .map_err(|e| anyhow::anyhow!("Frida Gum hook posix_spawn failed: {e:?}"))?;
        ORIG_POSIX_SPAWN = orig_spawn.0;

        let orig_exec = interceptor
            .replace_fast(
                NativePointer(execve_ptr),
                NativePointer(hook_execve as *mut c_void),
            )
            .map_err(|e| anyhow::anyhow!("Frida Gum hook execve failed: {e:?}"))?;
        ORIG_EXECVE = orig_exec.0;
        info!("Installed posix_spawn and execve hooks to sanitize child environments via Frida Gum");
    }
    Ok(())
}
