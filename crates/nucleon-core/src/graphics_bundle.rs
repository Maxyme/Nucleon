//! Common graphics bundle mechanics; backends retain their layout and ABI policies.
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn version(dir: &Path) -> Option<String> {
    let path = dir.join("version.txt");
    if !path.is_file() {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

pub(crate) fn file(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    path.is_file().then_some(path)
}

/// Select the first existing directory alias, retaining the final fallback even if absent.
pub(crate) fn directory(root: &Path, aliases: &[&str]) -> PathBuf {
    aliases
        .iter()
        .map(|alias| root.join(alias))
        .find(|path| path.is_dir())
        .unwrap_or_else(|| root.join(aliases.last().expect("directory aliases cannot be empty")))
}

pub(crate) fn find<T>(
    paths: impl IntoIterator<Item = PathBuf>,
    inspect: impl Fn(&Path) -> Option<T>,
) -> Option<T> {
    paths.into_iter().find_map(|path| inspect(&path))
}

/// Environment paths precede the saved selection. Callers append backend-specific defaults.
pub(crate) fn configured<T>(
    env_paths: &[PathBuf],
    saved: &Path,
    inspect: impl Fn(&Path) -> Option<T>,
) -> Option<T> {
    find(env_paths.iter().cloned(), &inspect).or_else(|| {
        if !saved.is_file() {
            return None;
        }
        let text = fs::read_to_string(saved).ok()?;
        inspect(Path::new(text.trim()))
    })
}

pub(crate) fn register<T>(
    path: &Path,
    saved: &Path,
    description: &str,
    inspect: impl Fn(&Path) -> Option<T>,
    root: impl Fn(&T) -> &Path,
) -> Result<T> {
    let resolved = std::path::absolute(path)?;
    let bundle = inspect(&resolved).with_context(|| {
        format!(
            "Invalid {description} installation at {}",
            resolved.display()
        )
    })?;
    if let Some(parent) = saved.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::fs_util::atomic_write_file(saved, root(&bundle).to_string_lossy().as_bytes())
        .with_context(|| format!("Failed to write {}", saved.display()))?;
    Ok(bundle)
}

pub(crate) fn clear(saved: &Path) -> Result<()> {
    crate::backend_files::remove(saved).map(|_| ())
}

/// Stage optional files only into an existing destination. Prefix creation and x86 fallback
/// are backend policies, deliberately kept at the call site.
pub(crate) fn stage<'a>(
    target: &Path,
    label: &str,
    files: impl IntoIterator<Item = (&'a str, Option<&'a Path>)>,
) -> Result<usize> {
    if !target.is_dir() {
        return Ok(0);
    }
    let mut count = 0;
    for (name, source) in files {
        if let Some(source) = source {
            let destination = target.join(name);
            crate::backend_files::copy(source, &destination).with_context(|| {
                format!(
                    "Failed to stage {label} {name} to {}",
                    destination.display()
                )
            })?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dll(root: &Path, name: &str, contents: &[u8]) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn configured_paths_keep_priority_and_invalid_registration_preserves_selection() {
        let temp = tempfile::tempdir().unwrap();
        let saved = temp.path().join("config/dxvk_path.txt");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        dll(&first, "release/x64/d3d11.dll", b"first");
        dll(&second, "d3d11.dll", b"second");
        let inspect = crate::dxvk::inspect_candidate_dir;
        let bundle = register(&first, &saved, "DXVK", inspect, |b| &b.root).unwrap();
        assert_eq!(bundle.root, first.join("release"));
        assert_eq!(configured(&[], &saved, inspect).unwrap().root, bundle.root);
        assert_eq!(
            configured(&[second.clone(), first], &saved, inspect)
                .unwrap()
                .root,
            second
        );
        let previous = fs::read(&saved).unwrap();
        assert!(
            register(&temp.path().join("missing"), &saved, "DXVK", inspect, |b| {
                &b.root
            })
            .is_err()
        );
        assert_eq!(fs::read(&saved).unwrap(), previous);
        clear(&saved).unwrap();
        clear(&saved).unwrap();
        fs::create_dir(&saved).unwrap();
        assert!(clear(&saved).is_err());
    }

    #[test]
    fn layout_priorities_and_backend_specific_staging_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // Official directory aliases precede legacy ones; a flat DLL is a final fallback.
        for name in ["x86_64-windows/d3d11.dll", "x64/d3d11.dll", "d3d11.dll"] {
            dll(root, name, name.as_bytes());
        }
        let dxvk = crate::dxvk::inspect_candidate_dir(root).unwrap();
        assert_eq!(dxvk.x64_d3d11, Some(root.join("x86_64-windows/d3d11.dll")));
        dll(root, "x32/d3d11.dll", b"32");
        let dxvk = crate::dxvk::inspect_candidate_dir(root).unwrap();
        let prefix = root.join("prefix");
        assert_eq!(
            crate::dxvk::stage_dxvk_into_prefix(&dxvk, &prefix).unwrap(),
            0
        );
        let sys32 = prefix.join("drive_c/windows/system32");
        fs::create_dir_all(&sys32).unwrap();
        assert_eq!(
            crate::dxvk::stage_dxvk_into_prefix(&dxvk, &prefix).unwrap(),
            2
        );
        assert_eq!(fs::read(sys32.join("d3d11.dll")).unwrap(), b"32");
        let wow64 = prefix.join("drive_c/windows/syswow64");
        fs::create_dir(&wow64).unwrap();
        crate::dxvk::stage_dxvk_into_prefix(&dxvk, &prefix).unwrap();
        assert_eq!(fs::read(wow64.join("d3d11.dll")).unwrap(), b"32");
        assert_eq!(
            fs::read(sys32.join("d3d11.dll")).unwrap(),
            b"x86_64-windows/d3d11.dll"
        );

        dll(root, "x32/ddraw.dll", b"ddraw");
        dll(root, "version.txt", b"  v1\n");
        let d7vk = crate::d7vk::inspect_candidate_dir(root).unwrap();
        assert_eq!(d7vk.version.as_deref(), Some("v1"));
        crate::d7vk::stage_d7vk_into_prefix(&d7vk, &prefix).unwrap();
        assert_eq!(fs::read(wow64.join("ddraw.dll")).unwrap(), b"ddraw");
        assert!(!sys32.join("ddraw.dll").exists());

        // VKD3D creates system32 but does not redirect x86 DLLs into it.
        dll(root, "x64/d3d12.dll", b"64");
        dll(root, "x86/d3d12.dll", b"32");
        let vkd3d = crate::vkd3d::inspect_candidate_dir(root).unwrap();
        let fresh = root.join("fresh");
        assert_eq!(
            crate::vkd3d::stage_vkd3d_proton_into_prefix(&vkd3d, &fresh).unwrap(),
            1
        );
        assert_eq!(
            fs::read(fresh.join("drive_c/windows/system32/d3d12.dll")).unwrap(),
            b"64"
        );
        assert!(!fresh.join("drive_c/windows/syswow64").exists());
    }
}
