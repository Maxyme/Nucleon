use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::sync::{LazyLock, RwLock};
use frida_gum::interceptor::Interceptor;
use frida_gum::NativePointer;
use log::info;

#[repr(C)]
pub struct InterposeTuple {
    pub replacement: *const (),
    pub replacee: *const (),
}

unsafe impl Sync for InterposeTuple {}

#[link_section = "__DATA,__interpose"]
#[used]
pub static INTERPOSE_OPEN: InterposeTuple = InterposeTuple {
    replacement: hook_open as *const (),
    replacee: libc::open as *const (),
};

#[link_section = "__DATA,__interpose"]
#[used]
pub static INTERPOSE_OPENAT: InterposeTuple = InterposeTuple {
    replacement: hook_openat as *const (),
    replacee: libc::openat as *const (),
};

#[link_section = "__DATA,__interpose"]
#[used]
pub static INTERPOSE_FOPEN: InterposeTuple = InterposeTuple {
    replacement: hook_fopen as *const (),
    replacee: libc::fopen as *const (),
};

pub static mut ORIG_OPEN: *mut c_void = std::ptr::null_mut();
pub static mut ORIG_OPENAT: *mut c_void = std::ptr::null_mut();
pub static mut ORIG_FOPEN: *mut c_void = std::ptr::null_mut();

thread_local! {
    static IN_WEBPATCH: Cell<bool> = const { Cell::new(false) };
}

static CACHED_PATCHES: LazyLock<RwLock<HashMap<String, Option<Vec<u8>>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

unsafe fn call_orig_open(path: *const c_char, oflag: c_int, mode: libc::mode_t) -> c_int {
    if !ORIG_OPEN.is_null() {
        let orig: extern "C" fn(*const c_char, c_int, c_uint) -> c_int = std::mem::transmute(ORIG_OPEN);
        orig(path, oflag, mode as c_uint)
    } else {
        let next = libc::dlsym(libc::RTLD_NEXT, c"open".as_ptr());
        if !next.is_null() {
            let orig: extern "C" fn(*const c_char, c_int, c_uint) -> c_int = std::mem::transmute(next);
            orig(path, oflag, mode as c_uint)
        } else {
            libc::open(path, oflag, mode as c_uint)
        }
    }
}

unsafe fn call_orig_openat(dirfd: c_int, path: *const c_char, oflag: c_int, mode: libc::mode_t) -> c_int {
    if !ORIG_OPENAT.is_null() {
        let orig: extern "C" fn(c_int, *const c_char, c_int, c_uint) -> c_int = std::mem::transmute(ORIG_OPENAT);
        orig(dirfd, path, oflag, mode as c_uint)
    } else {
        let next = libc::dlsym(libc::RTLD_NEXT, c"openat".as_ptr());
        if !next.is_null() {
            let orig: extern "C" fn(c_int, *const c_char, c_int, c_uint) -> c_int = std::mem::transmute(next);
            orig(dirfd, path, oflag, mode as c_uint)
        } else {
            libc::openat(dirfd, path, oflag, mode as c_uint)
        }
    }
}

unsafe fn call_orig_fopen(path: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    if !ORIG_FOPEN.is_null() {
        let orig: extern "C" fn(*const c_char, *const c_char) -> *mut libc::FILE = std::mem::transmute(ORIG_FOPEN);
        orig(path, mode)
    } else {
        let next = libc::dlsym(libc::RTLD_NEXT, c"fopen".as_ptr());
        if !next.is_null() {
            let orig: extern "C" fn(*const c_char, *const c_char) -> *mut libc::FILE = std::mem::transmute(next);
            orig(path, mode)
        } else {
            libc::fopen(path, mode)
        }
    }
}

fn get_patched_data(path: &str) -> Option<Vec<u8>> {
    if let Ok(cache) = CACHED_PATCHES.read() {
        if let Some(cached_opt) = cache.get(path) {
            return cached_opt.clone();
        }
    }

    let content = IN_WEBPATCH.with(|g| {
        g.set(true);
        let res = fs::read_to_string(path);
        g.set(false);
        res
    }).ok();

    let patched_bytes = match content {
        Some(c) => transform_js(&c).map(String::into_bytes),
        None => None,
    };

    if let Ok(mut cache) = CACHED_PATCHES.write() {
        cache.insert(path.to_string(), patched_bytes.clone());
    }

    patched_bytes
}

pub fn should_patch_file(path: &str) -> bool {
    path.contains("steamui") && path.contains("/chunk~") && path.ends_with(".js")
}

pub fn transform_js(content: &str) -> Option<String> {
    let mut modified = content.to_string();
    let mut changed = false;

    // 1. Enable Install button in PlayBar (_e returns s.UM "Install" for u.Ul.KR)
    let target_install_btn = "case u.Ul.jw:return s.UM;";
    let repl_install_btn = "case u.Ul.jw:case u.Ul.KR:return s.UM;";
    if modified.contains(target_install_btn) {
        modified = modified.replace(target_install_btn, repl_install_btn);
        changed = true;
    }

    // 2. Do not treat InvalidPlatform as permanently unavailable
    let target_perm_unavail = "case D.Ul.pd:case D.Ul.K5:case D.Ul.KR:case D.Ul.Mu:return!0";
    let repl_perm_unavail = "case D.Ul.pd:case D.Ul.K5:case D.Ul.Mu:return!0";
    if modified.contains(target_perm_unavail) {
        modified = modified.replace(target_perm_unavail, repl_perm_unavail);
        changed = true;
    }

    // 3. Make is_available_on_current_platform return true
    let target_avail_platform = "get is_available_on_current_platform(){return this.local_per_client_data&&this.local_per_client_data.is_available_on_current_platform}";
    let repl_avail_platform = "get is_available_on_current_platform(){return true}";
    if modified.contains(target_avail_platform) {
        modified = modified.replace(target_avail_platform, repl_avail_platform);
        changed = true;
    }

    // 4. Force is_invalid_os_type to false -> Enables Install button in Steam UI
    let target_invalid_os = "get is_invalid_os_type(){return this.most_available_per_client_data.is_invalid_os_type}";
    let repl_invalid_os = "get is_invalid_os_type(){return false}";
    if modified.contains(target_invalid_os) {
        modified = modified.replace(target_invalid_os, repl_invalid_os);
        changed = true;
    }

    // 5. Replace InvalidPlatform status text with "Playable via Steam Play (Nucleon)"
    let target_status_text = r##"(0,W.we)("#DisplayStatus_InvalidPlatform")"##;
    let repl_status_text = r##""Playable via Steam Play (Nucleon)""##;
    if modified.contains(target_status_text) {
        modified = modified.replace(target_status_text, repl_status_text);
        changed = true;
    }

    // 6. Do not exclude InvalidPlatform games from collection platform filter
    let target_filter = "r&&e.BIsPerClientDataLocal(r)&&r.display_status==ze.Ul.KR&&(t=!1)";
    let repl_filter = "false&&(t=!1)";
    if modified.contains(target_filter) {
        modified = modified.replace(target_filter, repl_filter);
        changed = true;
    }

    // 7. Enable Compatibility tab in Game Properties
    let target_compat = r##"(0,f.CI)()&&o.push({title:(0,A.we)("#AppProperties_CompatibilityPage")"##;
    let repl_compat = r##"true&&o.push({title:(0,A.we)("#AppProperties_CompatibilityPage")"##;
    if modified.contains(target_compat) {
        modified = modified.replace(target_compat, repl_compat);
        changed = true;
    }

    // 8. Always enable Compatibility tool force checkbox in Game Properties
    let target_compat_enabled = "()=>u.rV.settings.bCompatEnabled";
    let repl_compat_enabled = "()=>true";
    if modified.contains(target_compat_enabled) {
        modified = modified.replace(target_compat_enabled, repl_compat_enabled);
        changed = true;
    }

    // 9. Compatibility tab in Steam Settings
    let target_settings = "Compatibility:{visible:t&&(0,f.CI)()&&!(0,f.rf)()";
    let repl_settings = "Compatibility:{visible:t&&true&&!(0,f.rf)()";
    if modified.contains(target_settings) {
        modified = modified.replace(target_settings, repl_settings);
        changed = true;
    }

    // 10. Fallback global compat tool in Steam Settings dropdown
    let target_tool_default = "const t=(0,c.t0)().strCompatTool,";
    let repl_tool_default = r#"const t=(0,c.t0)().strCompatTool||(A.length?A[0].data:"nucleon"),"#;
    if modified.contains(target_tool_default) {
        modified = modified.replace(target_tool_default, repl_tool_default);
        changed = true;
    }

    // 11. SteamPlay section in Steam Settings
    if modified.contains("function ue(e){return(0,T.CI)()?") {
        modified = modified.replace("function ue(e){return(0,T.CI)()?", "function ue(e){return true?");
        changed = true;
    }

    // 12. Add Non-Steam EXE filter
    let target_exe = r##"("#AddNonSteam_Filter_Exe_MacOS"),rFilePatterns:["*.app"]"##;
    let repl_exe = r##"("#AddNonSteam_Filter_Exe_MacOS"),rFilePatterns:["*.app","*.exe"]"##;
    if modified.contains(target_exe) {
        modified = modified.replace(target_exe, repl_exe);
        changed = true;
    }

    // 13. Allow .exe in image / executable filters
    let target_img = r##"{strFileTypeName:"Image Files (*.tga,*.png)",rFilePatterns:["*.tga","*.png"]}"##;
    let repl_img = r##"{strFileTypeName:"Image Files (*.tga,*.png,*.exe)",rFilePatterns:["*.tga","*.png","*.exe"]}"##;
    if modified.contains(target_img) {
        modified = modified.replace(target_img, repl_img);
        changed = true;
    }

    // 14. Game list entry notice for Windows apps
    let target_entry = r##"("#GameList_Entry_Invalid_OSType2")"##;
    let repl_entry = r##""Enable Nucleon under Properties > Compatibility to install and run the Windows version.""##;
    if modified.contains(target_entry) {
        modified = modified.replace(target_entry, repl_entry);
        changed = true;
    }

    if changed {
        Some(modified)
    } else {
        None
    }
}

/// Interposes open() to inspect and transform Steam CEF WebUI chunks in memory.
///
/// # Safety
/// The caller must ensure that `path` is null or points to a valid null-terminated C string.
pub unsafe extern "C" fn hook_open(path: *const c_char, oflag: c_int, mode: libc::mode_t) -> c_int {
    if path.is_null() {
        return call_orig_open(path, oflag, mode);
    }

    if IN_WEBPATCH.with(|g| g.get()) {
        return call_orig_open(path, oflag, mode);
    }

    let path_str = CStr::from_ptr(path).to_string_lossy();
    if should_patch_file(&path_str) {
        if let Some(patched_bytes) = get_patched_data(&path_str) {
            let temp_res = IN_WEBPATCH.with(|g| {
                g.set(true);
                let res = tempfile::tempfile();
                g.set(false);
                res
            });
            if let Ok(mut temp) = temp_res {
                if temp.write_all(&patched_bytes).is_ok() && temp.seek(SeekFrom::Start(0)).is_ok() {
                    use std::os::unix::io::IntoRawFd;
                    return temp.into_raw_fd();
                }
            }
        }
    }

    call_orig_open(path, oflag, mode)
}

/// Interposes openat() to inspect and transform Steam CEF WebUI chunks in memory.
///
/// # Safety
/// The caller must ensure that `path` is null or points to a valid null-terminated C string.
pub unsafe extern "C" fn hook_openat(dirfd: c_int, path: *const c_char, oflag: c_int, mode: libc::mode_t) -> c_int {
    if path.is_null() {
        return call_orig_openat(dirfd, path, oflag, mode);
    }

    if IN_WEBPATCH.with(|g| g.get()) {
        return call_orig_openat(dirfd, path, oflag, mode);
    }

    let path_str = CStr::from_ptr(path).to_string_lossy();
    if should_patch_file(&path_str) {
        if let Some(patched_bytes) = get_patched_data(&path_str) {
            let temp_res = IN_WEBPATCH.with(|g| {
                g.set(true);
                let res = tempfile::tempfile();
                g.set(false);
                res
            });
            if let Ok(mut temp) = temp_res {
                if temp.write_all(&patched_bytes).is_ok() && temp.seek(SeekFrom::Start(0)).is_ok() {
                    use std::os::unix::io::IntoRawFd;
                    return temp.into_raw_fd();
                }
            }
        }
    }

    call_orig_openat(dirfd, path, oflag, mode)
}

/// Interposes fopen() to inspect and transform Steam CEF WebUI chunks in memory.
///
/// # Safety
/// The caller must ensure that `path` and `mode` are valid null-terminated C strings.
pub unsafe extern "C" fn hook_fopen(path: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    if path.is_null() || mode.is_null() {
        return call_orig_fopen(path, mode);
    }

    if IN_WEBPATCH.with(|g| g.get()) {
        return call_orig_fopen(path, mode);
    }

    let path_str = CStr::from_ptr(path).to_string_lossy();
    if should_patch_file(&path_str) {
        if let Some(patched_bytes) = get_patched_data(&path_str) {
            let temp_res = IN_WEBPATCH.with(|g| {
                g.set(true);
                let res = tempfile::tempfile();
                g.set(false);
                res
            });
            if let Ok(mut temp) = temp_res {
                if temp.write_all(&patched_bytes).is_ok() && temp.seek(SeekFrom::Start(0)).is_ok() {
                    use std::os::unix::io::IntoRawFd;
                    let fd = temp.into_raw_fd();
                    let fp = libc::fdopen(fd, mode);
                    if !fp.is_null() {
                        return fp;
                    }
                    libc::close(fd);
                }
            }
        }
    }

    call_orig_fopen(path, mode)
}

pub fn install_webpatch_hooks() -> Result<(), anyhow::Error> {
    let gum = crate::get_gum();
    let mut interceptor = Interceptor::obtain(gum);

    unsafe {
        let open_ptr = libc::open as *mut c_void;
        let orig_open = interceptor
            .replace_fast(
                NativePointer(open_ptr),
                NativePointer(hook_open as *mut c_void),
            )
            .map_err(|e| anyhow::anyhow!("Frida Gum hook open failed: {e:?}"))?;
        ORIG_OPEN = orig_open.0;

        let openat_ptr = libc::openat as *mut c_void;
        let orig_openat = interceptor
            .replace_fast(
                NativePointer(openat_ptr),
                NativePointer(hook_openat as *mut c_void),
            )
            .map_err(|e| anyhow::anyhow!("Frida Gum hook openat failed: {e:?}"))?;
        ORIG_OPENAT = orig_openat.0;

        let fopen_ptr = libc::fopen as *mut c_void;
        let orig_fopen = interceptor
            .replace_fast(
                NativePointer(fopen_ptr),
                NativePointer(hook_fopen as *mut c_void),
            )
            .map_err(|e| anyhow::anyhow!("Frida Gum hook fopen failed: {e:?}"))?;
        ORIG_FOPEN = orig_fopen.0;

        info!("Installed WebUI webpatch hooks (open, openat, fopen) via Frida Gum");
    }
    Ok(())
}
