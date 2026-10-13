use crate::{
    get_gum,
    scanner::{PinnedImage, snapshot_memory},
};
use anyhow::{Context, Result, bail, ensure};
use frida_gum::{NativePointer, interceptor::Interceptor};
use nucleon_core::steam_contract::ValidatedContract;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

type EligibilityQuery = unsafe extern "C" fn(*mut c_void, u32) -> bool;
static ELIGIBILITY_QUERY: OnceLock<EligibilityQuery> = OnceLock::new();
struct InstallationState {
    image: Option<PinnedImage>,
    restart_required: bool,
}
static INSTALLATION: Mutex<InstallationState> = Mutex::new(InstallationState {
    image: None,
    restart_required: false,
});

/// Safety: Frida invokes this only for the reviewed self/u32/bool entry point.
/// The original is published before transaction activation. We never dereference
/// self, discover tool pointers, or retain foreign object pointers.
unsafe extern "C" fn eligibility_override(manager: *mut c_void, appid: u32) -> bool {
    let Some(original) = ELIGIBILITY_QUERY.get() else {
        return false;
    };
    // SAFETY: the pinned reviewed image and trampoline have the declared ABI;
    // arguments are forwarded without dereferencing manager in Rust.
    let native_result = unsafe { original(manager, appid) };
    if native_result || appid == 0 {
        return native_result;
    }
    // No Rust panic may escape a native callback. Steam remains the fallback if
    // filesystem/appinfo inspection fails or panics.
    std::panic::catch_unwind(|| {
        nucleon_core::steam::native_detect::app_native_status(&appid.to_string())
            .map(|native| !native)
            .unwrap_or(native_result)
    })
    .unwrap_or(native_result)
}

unsafe extern "C" fn write_instruction(memory: *mut c_void, payload: *mut c_void) {
    // SAFETY: Gum supplies a writable four-byte patch span, and the caller keeps
    // its u32 payload alive until the synchronous patch callback completes.
    unsafe {
        memory
            .cast::<u32>()
            .write_unaligned(payload.cast::<u32>().read());
    }
}

fn patch_instruction(address: usize, instruction: u32) -> Result<()> {
    let mut payload = instruction;
    // SAFETY: caller resolved this address in a pinned, validated executable
    // section. Gum manages executable-page permissions and instruction caches.
    let result = unsafe {
        frida_gum_sys::gum_memory_patch_code(
            address as *mut c_void,
            4,
            Some(write_instruction),
            (&mut payload as *mut u32).cast(),
        )
    };
    ensure!(result != 0, "Gum instruction patch failed at {address:#x}");
    ensure!(
        snapshot_memory(address, 4)? == instruction.to_le_bytes(),
        "Instruction patch verification failed at {address:#x}"
    );
    Ok(())
}

/// Installs only after a full loaded-text snapshot matches the reviewed image.
/// Tools remain resolved by Steam's own mapping logic; no guessed array/stride
/// fallback is supplied. Select a Nucleon tool per game through Steam.
pub fn install_contract(image: PinnedImage, reviewed: &ValidatedContract) -> Result<()> {
    ensure!(
        cfg!(target_arch = "aarch64"),
        "This contract requires ARM64 Steam"
    );
    let mut installed = INSTALLATION
        .lock()
        .map_err(|_| anyhow::anyhow!("Installation lock poisoned"))?;
    ensure!(
        !installed.restart_required,
        "Incomplete installation requires a Steam restart"
    );
    if installed.image.is_some() {
        return Ok(());
    }
    ensure!(
        ELIGIBILITY_QUERY.get().is_none(),
        "A previous failed installation requires a Steam restart"
    );
    reviewed.validate_loaded_header(&image.header_snapshot()?)?;
    let text = snapshot_memory(
        image.address(reviewed.text_address(), reviewed.preferred_base())?,
        reviewed.text_size(),
    )?;
    reviewed.validate_loaded_text(&text)?;
    let query_address = image.address(
        reviewed.contract().eligibility.virtual_address,
        reviewed.preferred_base(),
    )?;
    let mut plan = Vec::new();
    for site in &reviewed.contract().patches {
        plan.push((
            image.address(site.virtual_address, reviewed.preferred_base())?,
            site.expected_word,
            site.replacement_word()?,
        ));
    }

    let mut interceptor = Interceptor::obtain(get_gum());
    interceptor.begin_transaction();
    // Transaction end below is unconditional on every Result path. Gum's
    // transaction controls detour publication; code-site patches are individually
    // verified and rolled back on failure, not claimed to be jointly atomic.
    let mut applied = Vec::new();
    let mut query_replaced = false;
    let result = (|| -> Result<()> {
        for (address, previous, replacement) in &plan {
            ensure!(
                snapshot_memory(*address, 4)? == previous.to_le_bytes(),
                "Instruction changed after image validation at {address:#x}"
            );
            // Include the site before writing: verification can fail after mutation.
            applied.push((*address, *previous));
            patch_instruction(*address, *replacement)?;
        }
        let pointer = interceptor
            .replace_fast(
                NativePointer(query_address as *mut c_void),
                NativePointer(eligibility_override as *mut c_void),
            )
            .map_err(|error| anyhow::anyhow!("Eligibility detour failed: {error:?}"))?;
        query_replaced = true;
        ensure!(!pointer.0.is_null(), "Gum returned a null trampoline");
        // SAFETY: the exact entry witness and contract declare this function ABI;
        // Gum returns the callable original for this same entry.
        let original: EligibilityQuery = unsafe { std::mem::transmute(pointer.0) };
        ELIGIBILITY_QUERY
            .set(original)
            .map_err(|_| anyhow::anyhow!("Original already published"))?;
        Ok(())
    })();
    let mut rollback_error = None;
    if result.is_err() {
        if query_replaced {
            interceptor.revert(NativePointer(query_address as *mut c_void));
        }
        for (address, previous) in applied.into_iter().rev() {
            if let Err(error) = patch_instruction(address, previous) {
                rollback_error = Some(error);
            }
        }
    }
    interceptor.end_transaction();
    if let Some(error) = rollback_error {
        // Retain the image when rollback is incomplete; no published pointer can dangle.
        installed.image = Some(image);
        installed.restart_required = true;
        bail!("Installation failed and rollback was incomplete: {error:#}; restart Steam");
    }
    result.context("Steam contract installation failed")?;
    installed.image = Some(image);
    log::info!(
        "Installed reviewed Steam contract for UUID {}",
        reviewed.contract().client.uuid
    );
    Ok(())
}
