use std::fs;
use std::path::Path;
use anyhow::{Context, Result};

pub fn generate_compatibilitytool_vdf(tool_name: &str, display_name: &str) -> String {
    format!(
r#""compatibilitytools"
{{
  "compat_tools"
  {{
    "{tool_name}"
    {{
      "install_path" "."
      "display_name" "{display_name}"
      "from_oslist" "windows"
      "to_oslist" "macos"
    }}
  }}
}}
"#
    )
}

pub fn generate_toolmanifest_vdf() -> String {
    r#""manifest"
{
  "version" "2"
  "commandline" "/run %verb%"
}
"#
    .to_string()
}

pub fn generate_run_script() -> String {
    r#"#!/bin/sh
# Nucleon compatibility tool runner shim
DIR="$(cd "$(dirname "$0")" && pwd)"
exec "$DIR/nucleon-runner" "$@"
"#
    .to_string()
}

pub fn write_tool_bundle(dir: &Path, tool_name: &str, display_name: &str, runner_bin: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;

    // 1. Stage runner binary
    let dst_runner = dir.join("nucleon-runner");
    if dst_runner.exists() {
        let _ = fs::remove_file(&dst_runner);
    }
    fs::copy(runner_bin, &dst_runner)
        .with_context(|| format!("Failed to stage runner into {}", dst_runner.display()))?;

    // 2. Write compatibilitytool.vdf
    let vdf_path = dir.join("compatibilitytool.vdf");
    fs::write(&vdf_path, generate_compatibilitytool_vdf(tool_name, display_name))
        .with_context(|| format!("Failed to write {}", vdf_path.display()))?;

    // 3. Write toolmanifest.vdf
    let manifest_path = dir.join("toolmanifest.vdf");
    fs::write(&manifest_path, generate_toolmanifest_vdf())
        .with_context(|| format!("Failed to write {}", manifest_path.display()))?;

    // 4. Write run script and chmod +x
    let run_path = dir.join("run");
    fs::write(&run_path, generate_run_script())
        .with_context(|| format!("Failed to write {}", run_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&run_path)?.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&run_path, perms);
    }

    Ok(())
}

pub fn write_compatibilitytool_vdf(target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = generate_compatibilitytool_vdf("nucleon", "Nucleon (Game Porting Toolkit 4)");
    fs::write(target, content)
        .with_context(|| format!("Failed to write compatibilitytool.vdf at {}", target.display()))?;
    Ok(())
}
