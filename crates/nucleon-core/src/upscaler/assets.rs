use super::{read, transaction};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::process::Command;
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
    if p.provider == "bridge" {
        ensure!(
            crate::runtime_checks::pe(Some(&root.join("nvngx.dll")), true),
            "NGX forwarder must be x64"
        );
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
    use sha2::{Digest, Sha256};
    let source = format!(
        "{:x}",
        Sha256::digest(include_bytes!("../../../../tools/upscalers/ngx_bridge.py"))
    );
    crate::staging::key(&[original_digest, &source])
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
        "Talos recipe requires 'nucleon upscaler install patcher'"
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
    fs::create_dir_all(root.join("plugins"))?;
    crate::fs_util::atomic_write_file(root.join("plugins/OptiPatcher.asi"), bytes)?;
    verify_patcher(root)?;
    Ok(())
}
/// Embedded source build tools keep installed CLIs independent of their original checkout.
pub fn install(
    root: &Path,
    provider: &str,
    archive: Option<&Path>,
    wine: Option<&Path>,
    source: Option<&Path>,
) -> Result<()> {
    ensure!(
        matches!(provider, "stock" | "metalfx" | "native" | "bridge"),
        "Choose stock, metalfx, native, bridge or patcher"
    );
    fs::create_dir_all(root)?;
    let _lock = transaction::lock(&root.join(".provider.lock"))?;
    let scripts = tempfile::tempdir_in(root)?;
    for (name, content) in [
        (
            "ngx_bridge.py",
            include_str!("../../../../tools/upscalers/ngx_bridge.py"),
        ),
        (
            "install.py",
            include_str!("../../../../tools/upscalers/install.py"),
        ),
        (
            "native.py",
            include_str!("../../../../tools/upscalers/native.py"),
        ),
        (
            "dependencies.json",
            include_str!("../../../../tools/upscalers/dependencies.json"),
        ),
        (
            "fsr4_depth_compat.h",
            include_str!("../../../../tools/upscalers/fsr4_depth_compat.h"),
        ),
        (
            "test_depth_compat.m",
            include_str!("../../../../tools/upscalers/test_depth_compat.m"),
        ),
    ] {
        fs::write(scripts.path().join(name), content)?;
    }
    let mut cmd = Command::new("python3");
    cmd.arg(scripts.path().join("install.py"))
        .arg(provider)
        .arg("--root")
        .arg(root);
    for (flag, path) in [
        ("--archive", archive),
        ("--wine", wine),
        ("--source", source),
    ] {
        if let Some(p) = path {
            cmd.arg(flag).arg(p.canonicalize()?);
        }
    }
    ensure!(
        cmd.status()?.success(),
        "Upscaler provider installation failed; no game was launched"
    );
    if provider == "bridge" {
        bridge_provider(
            root,
            &wine
                .context("Bridge provider requires Wine")?
                .join("lib/wine/x86_64-windows/nvngx-on-metalfx.dll"),
        )?;
    } else if provider != "native" {
        read_provider(&provider_path(root, provider)?)?;
    } else {
        let digest = crate::staging::digest(
            &wine
                .context("Native provider requires Wine")?
                .join("lib/external/D3DMetal.framework/D3DMetal"),
        )?;
        read_provider(
            &root
                .join("providers/native")
                .join(native_key(&digest)?)
                .join("provider.json"),
        )?;
    }
    Ok(())
}
