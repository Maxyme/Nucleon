use std::collections::HashSet;
use core_graphics::display::kCGNullWindowID;
use core_graphics::window::{
    kCGWindowListOptionAll, CGWindowListCopyWindowInfo,
};
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation::number::CFNumber;
use core_foundation::dictionary::CFDictionary;
use anyhow::{bail, Result};

#[derive(Debug, Clone)]
pub struct PresentedWindow {
    pub window_id: u32,
    pub pid: i32,
    pub owner_name: String,
    pub window_name: String,
    pub layer: i32,
    pub is_onscreen: bool,
    pub width: f64,
    pub height: f64,
}

pub fn check_window_presentation(target_pids: Option<&[i32]>) -> Result<Vec<PresentedWindow>> {
    let pid_set: Option<HashSet<i32>> = target_pids.map(|p| p.iter().copied().collect());

    let window_list = unsafe {
        CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID)
    };

    if window_list.is_null() {
        bail!("Failed to query WindowServer window list via CoreGraphics");
    }

    let array: core_foundation::array::CFArray = unsafe {
        core_foundation::array::CFArray::wrap_under_create_rule(window_list)
    };

    let mut presented = Vec::new();

    for i in 0..array.len() {
        let dict_ptr = array.get(i).unwrap();
        let dict: CFDictionary = unsafe {
            CFDictionary::wrap_under_get_rule(*dict_ptr as *mut _)
        };

        let get_string = |key: &str| -> String {
            let cf_key = CFString::new(key);
            if let Some(val) = dict.find(cf_key.as_CFTypeRef()) {
                let cf_str: CFString = unsafe { CFString::wrap_under_get_rule(*val as *mut _) };
                cf_str.to_string()
            } else {
                String::new()
            }
        };

        let get_i32 = |key: &str| -> i32 {
            let cf_key = CFString::new(key);
            if let Some(val) = dict.find(cf_key.as_CFTypeRef()) {
                let cf_num: CFNumber = unsafe { CFNumber::wrap_under_get_rule(*val as *mut _) };
                cf_num.to_i32().unwrap_or(0)
            } else {
                0
            }
        };

        let get_bool = |key: &str| -> bool {
            let cf_key = CFString::new(key);
            if let Some(val) = dict.find(cf_key.as_CFTypeRef()) {
                extern "C" {
                    fn CFBooleanGetValue(boolean: *const std::ffi::c_void) -> libc::c_uchar;
                }
                unsafe { CFBooleanGetValue(*val as *const _) != 0 }
            } else {
                false
            }
        };

        let pid = get_i32("kCGWindowOwnerPID");
        let owner = get_string("kCGWindowOwnerName");
        let name = get_string("kCGWindowName");
        let layer = get_i32("kCGWindowLayer");
        let onscreen = get_bool("kCGWindowIsOnscreen");
        let window_id = get_i32("kCGWindowNumber") as u32;

        let is_wine = owner.to_lowercase().contains("wine");
        let matches_pid = pid_set.as_ref().map(|s| s.contains(&pid)).unwrap_or(true);

        if (is_wine || matches_pid) && onscreen && (0..=30).contains(&layer) {
            presented.push(PresentedWindow {
                window_id,
                pid,
                owner_name: owner,
                window_name: name,
                layer,
                is_onscreen: onscreen,
                width: 0.0,
                height: 0.0,
            });
        }
    }

    Ok(presented)
}
