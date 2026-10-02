use std::ffi::{CStr, c_char, c_int, c_uint};
use std::fs;
use std::io::{Seek, SeekFrom, Write};

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

pub fn should_patch_file(path: &str) -> bool {
    path.contains("steamui") && path.contains("/chunk~") && path.ends_with(".js")
}

pub fn transform_js(content: &str) -> Option<String> {
    let mut modified = content.to_string();
    let mut changed = false;

    // 1. Enable EXE filter in Non-Steam file dialog on macOS
    let target1 = r##"("#AddNonSteam_Filter_Exe_MacOS"),rFilePatterns:["*.app"]"##;
    let repl1 = r##"("#AddNonSteam_Filter_Exe_MacOS"),rFilePatterns:["*.app","*.exe"]"##;
    if modified.contains(target1) {
        modified = modified.replace(target1, repl1);
        changed = true;
    }

    // 2. Allow .exe in image / executable filters
    let target2 = r##"{strFileTypeName:"Image Files (*.tga,*.png)",rFilePatterns:["*.tga","*.png"]}"##;
    let repl2 = r##"{strFileTypeName:"Image Files (*.tga,*.png,*.exe)",rFilePatterns:["*.tga","*.png","*.exe"]}"##;
    if modified.contains(target2) {
        modified = modified.replace(target2, repl2);
        changed = true;
    }

    // 3. Game list entry notice for Windows apps
    let target3 = r##"("#GameList_Entry_Invalid_OSType2")"##;
    let repl3 = r##""Enable Nucleon under Properties > Compatibility to install and run the Windows version.""##;
    if modified.contains(target3) {
        modified = modified.replace(target3, repl3);
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
        return libc::open(path, oflag, mode as c_uint);
    }

    let path_str = CStr::from_ptr(path).to_string_lossy();
    if should_patch_file(&path_str) {
        if let Ok(content) = fs::read_to_string(path_str.as_ref()) {
            if let Some(patched) = transform_js(&content) {
                if let Ok(mut temp) = tempfile::tempfile() {
                    if temp.write_all(patched.as_bytes()).is_ok() && temp.seek(SeekFrom::Start(0)).is_ok() {
                        use std::os::unix::io::IntoRawFd;
                        return temp.into_raw_fd();
                    }
                }
            }
        }
    }

    libc::open(path, oflag, mode as c_uint)
}

/// Interposes openat() to inspect and transform Steam CEF WebUI chunks in memory.
///
/// # Safety
/// The caller must ensure that `path` is null or points to a valid null-terminated C string.
pub unsafe extern "C" fn hook_openat(dirfd: c_int, path: *const c_char, oflag: c_int, mode: libc::mode_t) -> c_int {
    if path.is_null() {
        return libc::openat(dirfd, path, oflag, mode as c_uint);
    }

    let path_str = CStr::from_ptr(path).to_string_lossy();
    if should_patch_file(&path_str) {
        if let Ok(content) = fs::read_to_string(path_str.as_ref()) {
            if let Some(patched) = transform_js(&content) {
                if let Ok(mut temp) = tempfile::tempfile() {
                    if temp.write_all(patched.as_bytes()).is_ok() && temp.seek(SeekFrom::Start(0)).is_ok() {
                        use std::os::unix::io::IntoRawFd;
                        return temp.into_raw_fd();
                    }
                }
            }
        }
    }

    libc::openat(dirfd, path, oflag, mode as c_uint)
}
