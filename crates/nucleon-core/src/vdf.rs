use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

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
    generate_run_script_for_engine(None)
}

pub fn generate_run_script_for_engine(engine: Option<&str>) -> String {
    generate_run_script_full(engine, None)
}

pub fn generate_run_script_full(engine: Option<&str>, wine_path: Option<&Path>) -> String {
    let mut script = String::from("#!/bin/sh\n# Nucleon compatibility tool runner shim\nDIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n");
    if let Some(eng) = engine {
        script.push_str(&format!("export NUCLEON_ENGINE=\"{eng}\"\n"));
    }
    if let Some(wp) = wine_path {
        script.push_str(&format!("export NUCLEON_WINE_PATH=\"{}\"\n", wp.display()));
    }
    script.push_str("exec \"$DIR/nucleon-runner\" \"$@\"\n");
    script
}

pub fn write_tool_bundle(
    dir: &Path,
    tool_name: &str,
    display_name: &str,
    runner_bin: &Path,
) -> Result<()> {
    write_tool_bundle_with_engine(dir, tool_name, display_name, runner_bin, None)
}

pub fn write_tool_bundle_with_engine(
    dir: &Path,
    tool_name: &str,
    display_name: &str,
    runner_bin: &Path,
    engine: Option<&str>,
) -> Result<()> {
    write_tool_bundle_with_wine(dir, tool_name, display_name, runner_bin, engine, None)
}

pub fn write_tool_bundle_with_wine(
    dir: &Path,
    tool_name: &str,
    display_name: &str,
    runner_bin: &Path,
    engine: Option<&str>,
    wine_path: Option<&Path>,
) -> Result<()> {
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
    fs::write(
        &vdf_path,
        generate_compatibilitytool_vdf(tool_name, display_name),
    )
    .with_context(|| format!("Failed to write {}", vdf_path.display()))?;

    // 3. Write toolmanifest.vdf
    let manifest_path = dir.join("toolmanifest.vdf");
    fs::write(&manifest_path, generate_toolmanifest_vdf())
        .with_context(|| format!("Failed to write {}", manifest_path.display()))?;

    // 4. Write run script and chmod +x
    let run_path = dir.join("run");
    fs::write(&run_path, generate_run_script_full(engine, wine_path))
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
    let content =
        generate_compatibilitytool_vdf("nucleon", "Nucleon (Wine + Automatic Graphics Backend)");
    fs::write(target, content).with_context(|| {
        format!(
            "Failed to write compatibilitytool.vdf at {}",
            target.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_write_tool_bundle_with_kosmickrisp_engine() {
        let dir = tempdir().unwrap();
        let tool_dir = dir.path().join("nucleon-kosmickrisp");
        let runner_src = dir.path().join("dummy-runner");
        fs::write(&runner_src, "dummy").unwrap();

        write_tool_bundle_with_engine(
            &tool_dir,
            "nucleon-kosmickrisp",
            "Nucleon (Wine + Mesa KosmicKrisp Vulkan)",
            &runner_src,
            Some("kosmickrisp"),
        )
        .unwrap();

        let vdf_content = fs::read_to_string(tool_dir.join("compatibilitytool.vdf")).unwrap();
        assert!(vdf_content.contains(r#""nucleon-kosmickrisp""#));
        assert!(
            vdf_content.contains(r#""display_name" "Nucleon (Wine + Mesa KosmicKrisp Vulkan)""#)
        );
        assert!(vdf_content.contains(r#""from_oslist" "windows""#));
        assert!(vdf_content.contains(r#""to_oslist" "macos""#));

        let run_content = fs::read_to_string(tool_dir.join("run")).unwrap();
        assert!(run_content.contains(r#"export NUCLEON_ENGINE="kosmickrisp""#));
        assert!(run_content.contains(r#"exec "$DIR/nucleon-runner" "$@""#));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(tool_dir.join("run")).unwrap();
            assert_eq!(meta.permissions().mode() & 0o111, 0o111);
        }
    }
}
