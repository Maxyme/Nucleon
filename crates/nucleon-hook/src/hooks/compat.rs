use std::ffi::c_void;
use dobby_sys::hook;
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

pub fn install_compat_hooks(init_addr: usize, is_enabled_addr: usize) -> Result<(), i32> {
    unsafe {
        if init_addr != 0 {
            ORIG_COMPAT_INIT = hook(init_addr as *mut c_void, hook_compat_init as *mut c_void)?;
            info!("Hooked CCompatManager::Init at 0x{:x}", init_addr);
        }
        if is_enabled_addr != 0 {
            ORIG_IS_ENABLED = hook(is_enabled_addr as *mut c_void, hook_is_enabled as *mut c_void)?;
            info!("Hooked CCompatManager::BIsEnabled at 0x{:x}", is_enabled_addr);
        }
    }
    Ok(())
}
