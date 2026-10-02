use std::ffi::{CStr, c_char, c_void};
use frida_gum::interceptor::Interceptor;
use frida_gum::NativePointer;
use log::info;

pub static mut ORIG_COMPAT_INIT: *mut c_void = std::ptr::null_mut();
pub static mut ORIG_IS_ENABLED: *mut c_void = std::ptr::null_mut();
pub static mut ORIG_FIND_TOOL: *mut c_void = std::ptr::null_mut();

/// Inline detour for CCompatManager::Init.
///
/// # Safety
/// The caller must ensure that `this` points to a valid CCompatManager instance.
pub unsafe extern "C" fn hook_compat_init(this: *mut c_void, arg1: *mut c_void) -> *mut c_void {
    let orig_fn: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        std::mem::transmute(ORIG_COMPAT_INIT);
    let ret = orig_fn(this, arg1);

    // Force enable Steam Play in manager struct
    force_enable_compat_manager(this);
    ret
}

/// Inline detour for CCompatManager::BIsEnabled.
///
/// # Safety
/// The caller must ensure that `this` points to a valid CCompatManager instance.
pub unsafe extern "C" fn hook_is_enabled(this: *mut c_void, appid: u32) -> bool {
    let orig_fn: extern "C" fn(*mut c_void, u32) -> bool =
        std::mem::transmute(ORIG_IS_ENABLED);
    force_enable_compat_manager(this);
    let ret = orig_fn(this, appid);
    if !ret && appid != 0 {
        return true;
    }
    ret
}

/// Inline detour for CCompatManager::FindToolForTargetApp.
///
/// # Safety
/// The caller must ensure that `this` points to a valid CCompatManager instance.
pub unsafe extern "C" fn hook_find_tool(this: *mut c_void, appid: u32) -> *mut c_void {
    let orig_fn: extern "C" fn(*mut c_void, u32) -> *mut c_void =
        std::mem::transmute(ORIG_FIND_TOOL);
    let tool = orig_fn(this, appid);
    if !tool.is_null() || appid == 0 {
        return tool;
    }

    find_registered_nucleon_tool(this)
}

/// Finds the registered Nucleon compatibility tool entry in CCompatManager.
///
/// # Safety
/// Caller must pass a valid or null pointer `compat_mgr`.
pub unsafe fn find_registered_nucleon_tool(compat_mgr: *mut c_void) -> *mut c_void {
    if compat_mgr.is_null() {
        return std::ptr::null_mut();
    }
    let base = compat_mgr as *const u8;

    for &arr_off in &[0x320, 0x28, 0x318, 0x328] {
        let count_off = arr_off + 0x10;
        let array_ptr = *(base.add(arr_off) as *const *const u8);
        if array_ptr.is_null() || (array_ptr as usize) < 0x1000 {
            continue;
        }
        let count = *(base.add(count_off) as *const u32);
        if count == 0 || count > 64 {
            continue;
        }

        let stride = 0x130;
        for i in 0..count {
            let entry = array_ptr.add(i as usize * stride);
            for &name_off in &[0x40, 0x8, 0x30, 0x48] {
                let name_ptr = *(entry.add(name_off) as *const *const c_char);
                if !name_ptr.is_null() && (name_ptr as usize) > 0x1000 {
                    if let Ok(name) = CStr::from_ptr(name_ptr).to_str() {
                        if name == "nucleon" || name == "notproton" {
                            return entry as *mut c_void;
                        }
                    }
                }
            }
        }
    }

    std::ptr::null_mut()
}

/// Force-enables the compatibility manager state bit.
///
/// # Safety
/// The caller must ensure that `this` is null or points to a valid CCompatManager instance.
pub unsafe fn force_enable_compat_manager(this: *mut c_void) {
    if this.is_null() {
        return;
    }
    // Offset 0x7B0 is the enabled flag in modern SteamClient
    let flag_ptr = (this as usize + 0x7B0) as *mut bool;
    *flag_ptr = true;
}

#[cfg(target_arch = "aarch64")]
unsafe extern "C" fn patch_nop(mem: *mut c_void, _user_data: *mut c_void) {
    let ptr = mem as *mut u32;
    *ptr = 0xd503201f; // ARM64 NOP
}

#[cfg(not(target_arch = "aarch64"))]
unsafe extern "C" fn patch_nop(mem: *mut c_void, _user_data: *mut c_void) {
    let ptr = mem as *mut u8;
    *ptr = 0x90; // x86_64 NOP
}

pub fn install_compat_hooks(init_addr: usize, is_enabled_addr: usize, oslist_gate_addr: usize, find_tool_addr: usize) -> Result<(), anyhow::Error> {
    info!("install_compat_hooks: init_addr=0x{:x}, is_enabled_addr=0x{:x}, oslist_gate=0x{:x}, find_tool_addr=0x{:x}", init_addr, is_enabled_addr, oslist_gate_addr, find_tool_addr);
    let gum = crate::get_gum();
    let mut interceptor = Interceptor::obtain(gum);

    unsafe {
        if init_addr != 0 {
            info!("install_compat_hooks: hooking CCompatManager::Init at 0x{:x}...", init_addr);
            let orig = interceptor
                .replace_fast(
                    NativePointer(init_addr as *mut c_void),
                    NativePointer(hook_compat_init as *mut c_void),
                )
                .map_err(|e| anyhow::anyhow!("Frida Gum hook compat_init failed: {e:?}"))?;
            ORIG_COMPAT_INIT = orig.0;
            info!("Hooked CCompatManager::Init at 0x{:x} via Frida Gum", init_addr);
        }
        if is_enabled_addr != 0 {
            info!("install_compat_hooks: hooking CCompatManager::BIsEnabled at 0x{:x}...", is_enabled_addr);
            let orig = interceptor
                .replace_fast(
                    NativePointer(is_enabled_addr as *mut c_void),
                    NativePointer(hook_is_enabled as *mut c_void),
                )
                .map_err(|e| anyhow::anyhow!("Frida Gum hook is_enabled failed: {e:?}"))?;
            ORIG_IS_ENABLED = orig.0;
            info!("Hooked CCompatManager::BIsEnabled at 0x{:x} via Frida Gum", is_enabled_addr);
        }
        if find_tool_addr != 0 {
            info!("install_compat_hooks: hooking CCompatManager::FindToolForTargetApp at 0x{:x}...", find_tool_addr);
            let orig = interceptor
                .replace_fast(
                    NativePointer(find_tool_addr as *mut c_void),
                    NativePointer(hook_find_tool as *mut c_void),
                )
                .map_err(|e| anyhow::anyhow!("Frida Gum hook find_tool failed: {e:?}"))?;
            ORIG_FIND_TOOL = orig.0;
            info!("Hooked CCompatManager::FindToolForTargetApp at 0x{:x} via Frida Gum", find_tool_addr);
        }
        if oslist_gate_addr != 0 {
            info!("install_compat_hooks: patching oslist_gate at 0x{:x}...", oslist_gate_addr);
            let patch_len = if cfg!(target_arch = "aarch64") { 4 } else { 1 };
            let ok = frida_gum_sys::gum_memory_patch_code(
                oslist_gate_addr as *mut _,
                patch_len,
                Some(patch_nop),
                std::ptr::null_mut(),
            );
            if ok != 0 {
                info!("Instrumented CCompatManager::GetOSListOverrideForApp.oslist_gate at 0x{:x} with NOP", oslist_gate_addr);
            } else {
                log::warn!("Failed to patch oslist_gate at 0x{:x}", oslist_gate_addr);
            }
        }
    }
    info!("install_compat_hooks: complete!");
    Ok(())
}
