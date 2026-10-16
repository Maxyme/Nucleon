//! Explicit experimental Steam tool; its selection never follows global Wine defaults.
use crate::{
    fs_util::atomic_write_file,
    paths,
    runtime::{ExecutionMode, PrefixCompatibility, ResolvedRuntime},
    vdf,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const TOOL_ID: &str = "nucleon-fex";
pub const DISPLAY_NAME: &str = "Nucleon (ARM64/FEX — Experimental)";
pub const SELECTION_FILE: &str = "fex-runtime.json";
pub const SELECTION_ENV: &str = "NUCLEON_FEX_SELECTION";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    schema: u32,
    root: PathBuf,
    #[serde(default)]
    managed: bool,
    prefix: PrefixCompatibility,
}

pub fn load_selection(path: &Path) -> Result<ResolvedRuntime> {
    let selection: Selection = serde_json::from_slice(
        &fs::read(path)
            .with_context(|| format!("Read experimental runtime selection {}", path.display()))?,
    )
    .with_context(|| format!("Invalid experimental runtime selection {}", path.display()))?;
    ensure!(
        selection.schema == 1,
        "Unsupported experimental tool selection schema"
    );
    ensure!(
        !selection.managed
            || selection
                .root
                .join(crate::managed_runtime::RECEIPT)
                .is_file(),
        "Managed runtime installation is incomplete. Run 'nucleon wine fex verify' or reinstall the verified source"
    );
    let runtime = ResolvedRuntime::inspect(&selection.root)?;
    ensure!(
        runtime.mode == ExecutionMode::Arm64Fex
            && runtime.root == selection.root
            && runtime.prefix == selection.prefix,
        "Experimental runtime identity changed. Register the intended runtime again with 'nucleon wine fex register <runtime>'"
    );
    Ok(runtime)
}

pub fn register(runtime_root: &Path, runner_bin: &Path) -> Result<()> {
    register_at(
        runtime_root,
        runner_bin,
        &paths::steam_fex_compat_tools_dir(),
    )
}

pub fn register_at(runtime_root: &Path, runner_bin: &Path, target: &Path) -> Result<()> {
    let runtime = ResolvedRuntime::inspect(runtime_root)?;
    ensure!(
        runtime.mode == ExecutionMode::Arm64Fex,
        "The experimental tool requires an ARM64/FEX runtime descriptor"
    );
    let selection = Selection {
        schema: 1,
        managed: runtime.root.join(crate::managed_runtime::RECEIPT).is_file(),
        root: runtime.root,
        prefix: runtime.prefix,
    };
    if target.is_dir() && fs::read_dir(target)?.next().is_some() {
        let manifest = fs::read_to_string(target.join("compatibilitytool.vdf"))?;
        ensure!(
            manifest.contains(&format!("\"{TOOL_ID}\"")),
            "Refusing to replace an unrelated Steam tool at {}",
            target.display()
        );
    }
    let parent = target
        .parent()
        .context("Steam tool destination has no parent")?;
    fs::create_dir_all(parent)?;
    let staged = tempfile::Builder::new()
        .prefix(".nucleon-fex-")
        .tempdir_in(parent)?;
    vdf::write_tool_bundle(staged.path(), TOOL_ID, DISPLAY_NAME, runner_bin)?;
    atomic_write_file(
        staged.path().join(SELECTION_FILE),
        serde_json::to_vec_pretty(&selection)?,
    )?;
    atomic_write_file(staged.path().join("run"), vdf::generate_fex_run_script())?;
    crate::downloads::publish(staged.path(), target)?;
    Ok(())
}

pub fn refresh(runner_bin: &Path) -> Result<()> {
    let target = paths::steam_fex_compat_tools_dir();
    if target.join(SELECTION_FILE).is_file() {
        let runtime = load_selection(&target.join(SELECTION_FILE))?;
        register_at(&runtime.root, runner_bin, &target)?;
    }
    Ok(())
}

pub fn unregister() -> Result<()> {
    let target = paths::steam_fex_compat_tools_dir();
    if target.exists() {
        fs::remove_dir_all(&target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_pins_native_identity_and_preserves_previous_registration_on_failure() {
        let (temp, runtime) = crate::runtime::fixture(ExecutionMode::Arm64Fex);
        let runner = temp.path().join("runner");
        fs::write(&runner, b"runner fixture").unwrap();
        let target = temp.path().join("tools/nucleon-fex");
        register_at(&runtime.root, &runner, &target).unwrap();
        assert_eq!(
            load_selection(&target.join(SELECTION_FILE)).unwrap().root,
            runtime.root
        );
        assert!(
            fs::read_to_string(target.join("run"))
                .unwrap()
                .contains("NUCLEON_FEX_SELECTION")
        );
        let previous = fs::read(target.join(SELECTION_FILE)).unwrap();
        assert!(register_at(&runtime.root, &temp.path().join("missing-runner"), &target).is_err());
        assert_eq!(fs::read(target.join(SELECTION_FILE)).unwrap(), previous);
        let descriptor = runtime.root.join(crate::runtime::DESCRIPTOR);
        let contents = fs::read_to_string(&descriptor)
            .unwrap()
            .replace("abi=1", "abi=2");
        fs::write(descriptor, contents).unwrap();
        assert!(load_selection(&target.join(SELECTION_FILE)).is_err());
    }

    #[test]
    fn runtime_paths_are_json_data_and_do_not_appear_in_the_shell_script() {
        let (temp, runtime) = crate::runtime::fixture(ExecutionMode::Arm64Fex);
        let named = temp.path().join("path ' with $(commands) and `ticks`");
        std::os::unix::fs::symlink(&runtime.root, &named).unwrap();
        let runner = temp.path().join("runner");
        fs::write(&runner, b"fixture").unwrap();
        let target = temp.path().join("tool");
        register_at(&named, &runner, &target).unwrap();
        let script = fs::read_to_string(target.join("run")).unwrap();
        assert!(!script.contains("commands"));
        assert!(!script.contains("NUCLEON_WINE_PATH"));
    }
}
