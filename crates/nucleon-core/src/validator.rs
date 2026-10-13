use anyhow::{Result, bail};
#[cfg(target_os = "macos")]
use core_foundation::base::{CFType, TCFType};
#[cfg(target_os = "macos")]
use core_foundation::dictionary::CFDictionary;
#[cfg(target_os = "macos")]
use core_foundation::number::CFNumber;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
#[cfg(target_os = "macos")]
use core_graphics::display::kCGNullWindowID;
#[cfg(target_os = "macos")]
use core_graphics::window::{CGWindowListCopyWindowInfo, kCGWindowListOptionAll};
#[cfg(target_os = "macos")]
use std::collections::HashSet;

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

#[cfg(target_os = "macos")]
fn dictionary_value(dict: &CFDictionary, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    let value = *dict.find(key.as_CFTypeRef())?;
    if value.is_null() {
        return None;
    }
    // SAFETY: this dictionary originates from CGWindowListCopyWindowInfo (or
    // its CFDictionary bounds value), whose values are retained CF objects.
    // Retain as CFType first; each caller checks the runtime type before use.
    Some(unsafe { CFType::wrap_under_get_rule(value) })
}

#[cfg(target_os = "macos")]
pub fn check_window_presentation(target_pids: Option<&[i32]>) -> Result<Vec<PresentedWindow>> {
    let pid_set: Option<HashSet<i32>> = target_pids.map(|p| p.iter().copied().collect());

    // SAFETY: valid constant options; CoreGraphics returns an owned CFArray or null.
    let window_list =
        unsafe { CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID) };

    if window_list.is_null() {
        bail!("Failed to query WindowServer window list via CoreGraphics");
    }

    // SAFETY: non-null Create-rule result with CFType-valued elements. Ownership
    // is transferred once and released by the array wrapper.
    let array: core_foundation::array::CFArray<CFType> =
        unsafe { core_foundation::array::CFArray::wrap_under_create_rule(window_list) };

    let mut presented = Vec::new();

    for i in 0..array.len() {
        let Some(value) = array.get(i) else {
            continue;
        };
        let Some(dict) = value.downcast::<CFDictionary>() else {
            continue;
        };

        let get_string = |key: &str| -> String {
            dictionary_value(&dict, key)
                .and_then(|value| value.downcast::<CFString>())
                .map(|value| value.to_string())
                .unwrap_or_default()
        };

        let get_i32 = |key: &str| -> i32 {
            dictionary_value(&dict, key)
                .and_then(|value| value.downcast::<CFNumber>())
                .and_then(|value| value.to_i32())
                .unwrap_or(0)
        };

        let get_bool = |key: &str| -> bool {
            dictionary_value(&dict, key)
                .and_then(|value| value.downcast::<core_foundation::boolean::CFBoolean>())
                .map(bool::from)
                .unwrap_or(false)
        };

        let pid = get_i32("kCGWindowOwnerPID");
        let owner = get_string("kCGWindowOwnerName");
        let name = get_string("kCGWindowName");
        let layer = get_i32("kCGWindowLayer");
        let onscreen = get_bool("kCGWindowIsOnscreen");
        let window_id = get_i32("kCGWindowNumber") as u32;

        let is_wine = owner.to_lowercase().contains("wine");
        let matches_pid = pid_set
            .as_ref()
            .map(|s| s.contains(&pid))
            .unwrap_or(is_wine);
        let (width, height) = {
            if let Some(bounds) = dictionary_value(&dict, "kCGWindowBounds")
                .and_then(|value| value.downcast::<CFDictionary>())
            {
                let dimension = |name: &str| {
                    dictionary_value(&bounds, name)
                        .and_then(|value| value.downcast::<CFNumber>())
                        .and_then(|value| value.to_f64())
                        .unwrap_or(0.0)
                };
                (dimension("Width"), dimension("Height"))
            } else {
                (0.0, 0.0)
            }
        };

        if matches_pid && onscreen && (0..=30).contains(&layer) {
            presented.push(PresentedWindow {
                window_id,
                pid,
                owner_name: owner,
                window_name: name,
                layer,
                is_onscreen: onscreen,
                width,
                height,
            });
        }
    }

    Ok(presented)
}

#[cfg(not(target_os = "macos"))]
pub fn check_window_presentation(_target_pids: Option<&[i32]>) -> Result<Vec<PresentedWindow>> {
    bail!("Window presentation validation via WindowServer is only supported on macOS");
}
