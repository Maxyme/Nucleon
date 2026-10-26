use super::read;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub schema: u32,
    pub provider: String,
    pub version: String,
    pub files: BTreeMap<PathBuf, String>,
    pub framework_sha256: Option<String>,
}
pub(crate) fn read_provider(path: &Path) -> Result<Provider> {
    let p: Provider = read(path)?;
    ensure!(
        p.schema == 1 && !p.files.is_empty(),
        "Invalid provider manifest"
    );
    let root = path.parent().unwrap();
    for (relative, expected) in &p.files {
        ensure!(
            !relative.is_absolute()
                && relative
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Unsafe provider file"
        );
        ensure!(
            crate::staging::digest(&root.join(relative))? == *expected,
            "Provider file changed: {}. Reinstall the provider",
            relative.display()
        );
    }
    let required: &[&str] = match p.provider.as_str() {
        "bridge" => &["nvngx.dll"],
        "native" => &[], // Native manifests also cover external shader compiler identities.
        "stock" | "metalfx" => &["OptiScaler.dll", "OptiScaler.ini"],
        _ => anyhow::bail!("Unknown provider manifest"),
    };
    ensure!(
        required
            .iter()
            .all(|name| p.files.contains_key(Path::new(name))),
        "Provider manifest omits required files"
    );
    if p.provider == "bridge" {
        let bytes = fs::read(root.join("nvngx.dll"))?;
        let names = super::ngx::exports(&bytes)?;
        super::ngx::verify_forwarder(&bytes, &names)?;
    } else if p.provider != "native" {
        ensure!(
            crate::runtime_checks::pe(Some(&root.join("OptiScaler.dll")), true),
            "Provider is not an ordinary x64 OptiScaler DLL"
        );
    }
    Ok(p)
}
pub(crate) fn catalog() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../../tools/upscalers/dependencies.json"
    ))
    .expect("Pinned upscaler catalog")
}
pub(crate) fn native_key(original_digest: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    let native = format!(
        "{:x}",
        Sha256::digest(include_bytes!("../../../../tools/upscalers/native.py"))
    );
    let adapter = format!(
        "{:x}",
        Sha256::digest(include_bytes!(
            "../../../../tools/upscalers/fsr4_depth_compat.h"
        ))
    );
    crate::staging::key(&[
        original_digest,
        catalog()["fsr4"]["commit"].as_str().unwrap(),
        &native,
        &adapter,
    ])
}
pub(crate) fn bridge_key(original_digest: &str) -> Result<String> {
    // Preserve the identity of existing ngx-forward-v1 installations. This is the
    // SHA-256 of the retired Python builder; the Rust builder emits the same ABI.
    const BUILDER_V1: &str = "cfe24edc4b0056c1abc42f95b93ff28fd2bf6d13aaf57b0a7caa5b727fbbcc41";
    crate::staging::key(&[original_digest, BUILDER_V1])
}
pub(crate) fn bridge_provider(root: &Path, bridge: &Path) -> Result<PathBuf> {
    let key = bridge_key(&crate::staging::digest(bridge)?)?;
    let manifest = root
        .join("providers/bridge")
        .join(key)
        .join("provider.json");
    ensure!(
        read_provider(&manifest)?.provider == "bridge",
        "Install the matching NGX forwarding bridge first"
    );
    Ok(manifest)
}
pub(crate) fn provider_path(root: &Path, provider: &str) -> Result<PathBuf> {
    ensure!(matches!(provider, "stock" | "metalfx"), "Unknown provider");
    let spec = catalog();
    let key = if provider == "stock" {
        "stock"
    } else {
        "optiscaler"
    };
    Ok(root
        .join("providers")
        .join(provider)
        .join(spec[key]["sha256"].as_str().unwrap())
        .join("provider.json"))
}
pub(crate) fn verify_patcher(root: &Path) -> Result<PathBuf> {
    let path = root.join("plugins/OptiPatcher.asi");
    ensure!(
        path.is_file(),
        "This game requires 'nucleon upscaler install patcher'"
    );
    ensure!(
        crate::staging::digest(&path)? == catalog()["optipatcher"]["sha256"].as_str().unwrap(),
        "OptiPatcher checksum mismatch"
    );
    ensure!(
        crate::runtime_checks::pe(Some(&path), true),
        "OptiPatcher must be x64"
    );
    Ok(path)
}
pub fn install_patcher(root: &Path, source: Option<&Path>) -> Result<()> {
    let c = catalog();
    let spec = &c["optipatcher"];
    let bytes = match source {
        Some(p) => fs::read(p)?,
        None => {
            let mut file =
                crate::downloads::download(spec["url"].as_str().unwrap(), 16 * 1024 * 1024)?;
            let mut bytes = vec![];
            std::io::Read::read_to_end(&mut file, &mut bytes)?;
            bytes
        }
    };
    use sha2::{Digest, Sha256};
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == spec["sha256"].as_str().unwrap(),
        "OptiPatcher checksum mismatch"
    );
    ensure!(
        crate::runtime_checks::pe_abi(&bytes)? == crate::runtime_checks::PeAbi::X64,
        "OptiPatcher must be x64"
    );
    fs::create_dir_all(root.join("plugins"))?;
    crate::fs_util::atomic_write_file(root.join("plugins/OptiPatcher.asi"), bytes)?;
    verify_patcher(root)?;
    Ok(())
}
