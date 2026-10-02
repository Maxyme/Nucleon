use std::ffi::c_void;
use frida_gum::interceptor::Interceptor;
use frida_gum::{Gum, NativePointer};
use log::info;

pub static mut ORIG_COMPAT_INIT: *mut c_void = std::ptr::null_mut();
pub static mut ORIG_IS_ENABLED: *mut c_void = std::ptr::null_mut();

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
    orig_fn(this, appid)
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

pub fn install_compat_hooks(init_addr: usize, is_enabled_addr: usize) -> Result<(), anyhow::Error> {
    let gum = Gum::obtain();
    let mut interceptor = Interceptor::obtain(&gum);

    unsafe {
        if init_addr != 0 {
            let orig = interceptor
                .replace_fast(
                    NativePointer(init_addr as *mut c_void),
                    NativePointer(hook_compat_init as *mut c_void),
                )
                .map_err(|e| anyhow::anyhow!("Frida Gum hook compat_init failed: {e:?}"))?;
            ORIG_COMPAT_INIT = orig.0;
            info!("Hooked CCompatManager::Init at 0x{:x} via Frida Gum (trampoline at {:p})", init_addr, orig.0);
        }
        if is_enabled_addr != 0 {
            let orig = interceptor
                .replace_fast(
                    NativePointer(is_enabled_addr as *mut c_void),
                    NativePointer(hook_is_enabled as *mut c_void),
                )
                .map_err(|e| anyhow::anyhow!("Frida Gum hook is_enabled failed: {e:?}"))?;
            ORIG_IS_ENABLED = orig.0;
            info!("Hooked CCompatManager::BIsEnabled at 0x{:x} via Frida Gum (trampoline at {:p})", is_enabled_addr, orig.0);
        }
    }
    Ok(())
}
