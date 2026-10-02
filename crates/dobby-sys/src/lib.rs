use std::ffi::{c_char, c_int, c_void, CString};

extern "C" {
    pub fn DobbyHook(
        address: *mut c_void,
        replace_call: *mut c_void,
        origin_call: *mut *mut c_void,
    ) -> c_int;

    pub fn DobbyInstrument(
        address: *mut c_void,
        pre_handler: extern "C" fn(*mut c_void, *mut c_void),
    ) -> c_int;

    pub fn DobbySymbolResolver(
        image_name: *const c_char,
        symbol_name: *const c_char,
    ) -> *mut c_void;
}

/// Hook a function pointer, returning the original trampoline pointer if successful.
///
/// # Safety
/// The caller must ensure that `target` points to valid executable machine code
/// and that `detour` has the compatible calling convention and signature as `target`.
pub unsafe fn hook(target: *mut c_void, detour: *mut c_void) -> Result<*mut c_void, i32> {
    let mut origin: *mut c_void = std::ptr::null_mut();
    let rc = DobbyHook(target, detour, &mut origin);
    if rc == 0 {
        Ok(origin)
    } else {
        Err(rc)
    }
}

/// Resolve a symbol in an image by name using Dobby's Mach-O symbol resolver.
pub fn resolve_symbol(image: Option<&str>, symbol: &str) -> Option<*mut c_void> {
    let c_sym = CString::new(symbol).ok()?;
    let c_image = image.and_then(|img| CString::new(img).ok());
    let img_ptr = c_image.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null());

    let res = unsafe { DobbySymbolResolver(img_ptr, c_sym.as_ptr()) };
    if res.is_null() {
        None
    } else {
        Some(res)
    }
}
