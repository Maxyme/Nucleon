use std::ffi::{CStr, c_char, c_void};

pub struct MachOImage {
    pub name: String,
    pub header: *const c_void,
    pub slide: isize,
}

extern "C" {
    fn _dyld_image_count() -> u32;
    fn _dyld_get_image_name(image_index: u32) -> *const c_char;
    fn _dyld_get_image_header(image_index: u32) -> *const c_void;
    fn _dyld_get_image_vmaddr_slide(image_index: u32) -> libc::intptr_t;
}

pub fn find_image(substring: &str) -> Option<MachOImage> {
    unsafe {
        let count = _dyld_image_count();
        for i in 0..count {
            let name_ptr = _dyld_get_image_name(i);
            if name_ptr.is_null() {
                continue;
            }
            let name = CStr::from_ptr(name_ptr).to_string_lossy();
            if name.contains(substring) {
                let header = _dyld_get_image_header(i);
                let slide = _dyld_get_image_vmaddr_slide(i) as isize;
                return Some(MachOImage {
                    name: name.to_string(),
                    header,
                    slide,
                });
            }
        }
    }
    None
}

/// Matches an Array of Bytes (AOB) with ?? wildcards against memory
pub fn aob_scan(base: *const u8, size: usize, pattern: &str) -> Option<*const u8> {
    let tokens: Vec<&str> = pattern.split_whitespace().collect();
    if tokens.is_empty() || size < tokens.len() {
        return None;
    }

    let mut bytes = Vec::new();
    let mut mask = Vec::new();

    for t in tokens {
        if t == "??" || t == "?" {
            bytes.push(0u8);
            mask.push(false);
        } else if let Ok(b) = u8::from_str_radix(t, 16) {
            bytes.push(b);
            mask.push(true);
        } else {
            return None;
        }
    }

    let pat_len = bytes.len();
    let slice = unsafe { std::slice::from_raw_parts(base, size) };

    for i in 0..=(size - pat_len) {
        let mut matched = true;
        for j in 0..pat_len {
            if mask[j] && slice[i + j] != bytes[j] {
                matched = false;
                break;
            }
        }
        if matched {
            return Some(unsafe { base.add(i) });
        }
    }

    None
}
