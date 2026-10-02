/// Path to the compiled overlay-shim.dylib built during build.rs
pub const OVERLAY_SHIM_DYLIB: &str = env!("OVERLAY_SHIM_DYLIB");

/// Raw bytes of overlay-shim.dylib embedded into the binary for deployment
pub const OVERLAY_SHIM_BYTES: &[u8] = include_bytes!(env!("OVERLAY_SHIM_DYLIB"));
