use anyhow::{Context, Result, ensure};
use std::ffi::CString;
use std::ffi::{CStr, c_void};

/// Owns a loader reference obtained before inspecting any borrowed image data.
/// Metadata is immutable and cannot be forged through the public API.
pub struct PinnedImage {
    path: std::path::PathBuf,
    header_address: usize,
    handle: usize,
}

impl PinnedImage {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub fn header_snapshot(&self) -> Result<Vec<u8>> {
        let header = snapshot_memory(self.header_address, 32)?;
        ensure!(
            u32::from_le_bytes(header[0..4].try_into()?) == 0xfeed_facf,
            "Pinned image is not native little-endian Mach-O 64"
        );
        let commands = u32::from_le_bytes(header[20..24].try_into()?) as usize;
        ensure!(
            commands <= 1024 * 1024,
            "Unreasonable image load-command size"
        );
        snapshot_memory(self.header_address, 32 + commands)
    }
    pub fn address(&self, virtual_address: u64, preferred_base: u64) -> Result<usize> {
        let offset = virtual_address
            .checked_sub(preferred_base)
            .context("Address before image")?;
        self.header_address
            .checked_add(usize::try_from(offset)?)
            .context("Image address overflow")
    }
}

impl Drop for PinnedImage {
    fn drop(&mut self) {
        // SAFETY: this reference was obtained once by dlopen and is owned here.
        unsafe {
            libc::dlclose(self.handle as *mut c_void);
        }
    }
}

pub fn pin_steam_client() -> Result<Option<PinnedImage>> {
    let candidates = [
        nucleon_core::paths::steam_data_dir()
            .join("Steam.AppBundle/Steam/Contents/MacOS/steamclient.dylib"),
        nucleon_core::paths::steam_app().join("Contents/MacOS/steamclient.dylib"),
    ];
    for path in candidates {
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        let cpath = CString::new(path.as_os_str().as_encoded_bytes())?;
        // SAFETY: valid path; NOLOAD pins an existing image and never loads Steam.
        let handle = unsafe { libc::dlopen(cpath.as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD) };
        if handle.is_null() {
            continue;
        }
        let mut pinned = PinnedImage {
            path,
            header_address: 0,
            handle: handle as usize,
        };
        // SAFETY: handle owns a live loader reference; symbol string is terminated.
        let export = unsafe { libc::dlsym(handle, c"CreateInterface".as_ptr()) };
        ensure!(
            !export.is_null(),
            "Pinned Steam client lacks CreateInterface"
        );
        // SAFETY: dladdr initializes this POD structure, and the export is pinned.
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        ensure!(
            unsafe { libc::dladdr(export, &mut info) } != 0,
            "Cannot resolve pinned export"
        );
        ensure!(
            !info.dli_fname.is_null() && !info.dli_fbase.is_null(),
            "Missing pinned image metadata"
        );
        // SAFETY: dladdr's image name remains valid under the owned loader reference.
        let owner = unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes();
        let owner = std::path::Path::new(std::ffi::OsStr::new(std::str::from_utf8(owner)?))
            .canonicalize()?;
        ensure!(
            owner == pinned.path,
            "CreateInterface belongs to a different image"
        );
        pinned.header_address = info.dli_fbase as usize;
        return Ok(Some(pinned));
    }
    Ok(None)
}

/// Copy through the kernel, avoiding a Rust slice borrowing concurrently patched
/// native executable memory. Bad/unmapped addresses produce a kernel error.
#[cfg(target_os = "macos")]
pub fn snapshot_memory(address: usize, length: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0u8; length];
    let mut copied = 0u64;
    // SAFETY: the output vector is initialized and writable for length bytes;
    // copied points to a writable u64. The kernel validates the source mapping.
    let result = unsafe {
        mach2::vm::mach_vm_read_overwrite(
            mach2::traps::mach_task_self(),
            address as u64,
            length as u64,
            bytes.as_mut_ptr() as u64,
            &mut copied,
        )
    };
    ensure!(
        result == 0 && copied == length as u64,
        "Could not snapshot native executable memory"
    );
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
pub fn snapshot_memory(_address: usize, _length: usize) -> Result<Vec<u8>> {
    anyhow::bail!("Loaded-image snapshots require macOS")
}

/// Matches an Array of Bytes (AOB) with wildcards against an owned/borrowed slice.
pub fn aob_scan_bytes(data: &[u8], pattern: &str) -> Option<usize> {
    let pattern: Vec<Option<u8>> = pattern
        .split_whitespace()
        .map(|token| match token {
            "?" | "??" => Some(None),
            _ => u8::from_str_radix(token, 16).ok().map(Some),
        })
        .collect::<Option<_>>()?;
    if pattern.is_empty() || pattern.len() > data.len() {
        return None;
    }
    data.windows(pattern.len()).position(|window| {
        window
            .iter()
            .zip(&pattern)
            .all(|(byte, expected)| expected.is_none_or(|expected| *byte == expected))
    })
}
