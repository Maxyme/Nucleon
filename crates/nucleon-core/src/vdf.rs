use anyhow::{Context, Result, ensure};
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

pub fn generate_fex_run_script() -> &'static str {
    "#!/bin/sh\nDIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nexport NUCLEON_FEX_SELECTION=\"$DIR/fex-runtime.json\"\nexec \"$DIR/nucleon-runner\" \"$@\"\n"
}

pub fn generate_run_script_for_engine(engine: Option<&str>) -> String {
    generate_run_script_full(engine, None)
}

pub fn generate_run_script_full(engine: Option<&str>, wine_path: Option<&Path>) -> String {
    let mut script = String::from(
        "#!/bin/sh\n# Nucleon compatibility tool runner shim\nDIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n",
    );
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
    let target = std::path::absolute(dir)?;
    let previous = match fs::symlink_metadata(&target) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.is_symlink(),
                "Steam tool destination must be a directory, not a file or symlink: {}",
                target.display()
            );
            Some(metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("Inspect Steam tool destination"),
    };
    let parent = target
        .parent()
        .context("Steam tool destination has no parent")?;
    fs::create_dir_all(parent)?;
    let staged = tempfile::Builder::new()
        .prefix(".nucleon-tool-")
        .tempdir_in(parent)?;
    if previous.is_some() {
        copy_tool_extras(&target, staged.path())?;
    }
    write_tool_bundle_contents(
        staged.path(),
        tool_name,
        display_name,
        runner_bin,
        engine,
        wine_path,
    )?;

    // TempDir starts private; published tools keep their directory permissions.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            staged.path(),
            previous
                .map(|metadata| metadata.permissions())
                .unwrap_or_else(|| fs::Permissions::from_mode(0o755)),
        )
        .context("Set Steam tool directory permissions")?;
    }

    // An exchange leaves the previous generation at staged.path(). TempDir
    // removes it only after the complete new bundle has been published.
    crate::downloads::publish(staged.path(), &target)
}

const TOOL_FILES: &[&str] = &[
    "nucleon-runner",
    "compatibilitytool.vdf",
    "toolmanifest.vdf",
    "run",
];

/// Preserve settings and custom helpers without following links or copying the
/// four managed files that this update will replace.
fn copy_tool_extras(source: &Path, target: &Path) -> Result<()> {
    let mut directories = Vec::new();
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() != 1 || !TOOL_FILES.iter().any(|name| entry.file_name() == *name)
        })
    {
        let entry = entry.with_context(|| format!("Read Steam tool {}", source.display()))?;
        let output = target.join(entry.path().strip_prefix(source)?);
        if entry.file_type().is_dir() {
            fs::create_dir(&output)?;
            directories.push((output, entry.metadata()?.permissions()));
        } else if entry.file_type().is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, output)?;
            #[cfg(not(unix))]
            anyhow::bail!("Steam tool symlinks require Unix");
        } else {
            ensure!(
                entry.file_type().is_file(),
                "Unsupported Steam tool entry: {}",
                entry.path().display()
            );
            fs::copy(entry.path(), &output)
                .with_context(|| format!("Preserve Steam tool file {}", entry.path().display()))?;
        }
    }
    // Preserve read-only directory modes after their children have been copied.
    for (directory, permissions) in directories.into_iter().rev() {
        fs::set_permissions(directory, permissions)?;
    }
    Ok(())
}

fn write_tool_bundle_contents(
    dir: &Path,
    tool_name: &str,
    display_name: &str,
    runner_bin: &Path,
    engine: Option<&str>,
    wine_path: Option<&Path>,
) -> Result<()> {
    // 1. Stage runner binary
    let dst_runner = dir.join("nucleon-runner");
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

    // 4. Write run script and make both entry points executable before publication.
    let run_path = dir.join("run");
    fs::write(&run_path, generate_run_script_full(engine, wine_path))
        .with_context(|| format!("Failed to write {}", run_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&run_path, &dst_runner] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .with_context(|| format!("Set executable permissions on {}", path.display()))?;
        }
    }

    for path in [&vdf_path, &manifest_path] {
        let contents = fs::read_to_string(path)?;
        keyvalues_parser::parse(&contents)
            .with_context(|| format!("Invalid staged Steam manifest {}", path.display()))?;
    }
    ensure!(
        fs::metadata(&dst_runner)?.len() != 0,
        "Steam tool runner is empty"
    );
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

    fn installed_tool(root: &Path) -> std::path::PathBuf {
        let runner = root.join("runner-source");
        fs::write(&runner, b"#!/bin/sh\nprintf 'working\\n'\n").unwrap();
        let tool = root.join("tool");
        write_tool_bundle_with_engine(&tool, "nucleon-test", "Working tool", &runner, Some("auto"))
            .unwrap();
        tool
    }

    fn bundle_contents(tool: &Path) -> Vec<Vec<u8>> {
        TOOL_FILES
            .iter()
            .map(|name| fs::read(tool.join(name)).unwrap())
            .collect()
    }

    fn no_staging_directories(root: &Path) {
        assert!(fs::read_dir(root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".nucleon-tool-")
        }));
    }

    #[test]
    fn missing_replacement_preserves_the_entire_working_bundle() {
        let temp = tempdir().unwrap();
        let tool = installed_tool(temp.path());
        let previous = bundle_contents(&tool);
        fs::write(tool.join("settings.json"), b"private settings").unwrap();

        let error = write_tool_bundle_with_engine(
            &tool,
            "nucleon-test",
            "Replacement tool",
            &temp.path().join("missing-runner"),
            Some("dxmt"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Failed to stage runner"));
        assert_eq!(bundle_contents(&tool), previous);
        assert_eq!(
            fs::read(tool.join("settings.json")).unwrap(),
            b"private settings"
        );
        #[cfg(unix)]
        {
            let output = std::process::Command::new(tool.join("run"))
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"working\n");
        }
        no_staging_directories(temp.path());
    }

    #[test]
    fn failed_initial_install_does_not_publish_an_incomplete_tool() {
        let temp = tempdir().unwrap();
        let tool = temp.path().join("tool");
        assert!(
            write_tool_bundle(&tool, "nucleon-test", "Test", &temp.path().join("missing")).is_err()
        );
        assert!(!tool.exists());
        no_staging_directories(temp.path());
    }

    #[test]
    fn invalid_staged_runner_leaves_all_installed_files_unchanged() {
        let temp = tempdir().unwrap();
        let tool = installed_tool(temp.path());
        let previous = bundle_contents(&tool);
        let runner = temp.path().join("empty-runner");
        fs::write(&runner, b"").unwrap();
        let error = write_tool_bundle_with_engine(
            &tool,
            "nucleon-test",
            "Replacement tool",
            &runner,
            Some("dxmt"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("runner is empty"));
        assert_eq!(bundle_contents(&tool), previous);
        no_staging_directories(temp.path());
    }

    #[cfg(unix)]
    #[test]
    fn successful_update_swaps_the_bundle_and_preserves_custom_files_links_and_modes() {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        let temp = tempdir().unwrap();
        let tool = installed_tool(temp.path());
        let previous_inode = fs::metadata(&tool).unwrap().ino();
        let mut previous_runner = fs::File::open(tool.join("nucleon-runner")).unwrap();
        let previous_bytes = fs::read(tool.join("nucleon-runner")).unwrap();
        let settings = tool.join("settings/profile.json");
        fs::create_dir(settings.parent().unwrap()).unwrap();
        fs::write(&settings, b"custom settings").unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(
            settings.parent().unwrap(),
            fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o750)).unwrap();
        let outside = temp.path().join("outside-helper");
        fs::write(&outside, b"untouched").unwrap();
        symlink(&outside, tool.join("custom-helper")).unwrap();
        symlink("settings/profile.json", tool.join("relative-settings")).unwrap();
        symlink("missing", tool.join("broken-link")).unwrap();

        let replacement = temp.path().join("replacement-runner");
        fs::write(&replacement, b"new runner").unwrap();
        write_tool_bundle_with_engine(
            &tool,
            "nucleon-test",
            "New tool",
            &replacement,
            Some("dxmt"),
        )
        .unwrap();

        assert_ne!(fs::metadata(&tool).unwrap().ino(), previous_inode);
        let mut bytes = Vec::new();
        previous_runner.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, previous_bytes);
        assert_eq!(
            fs::read(tool.join("nucleon-runner")).unwrap(),
            b"new runner"
        );
        assert!(
            fs::read_to_string(tool.join("run"))
                .unwrap()
                .contains("dxmt")
        );
        assert!(
            fs::read_to_string(tool.join("compatibilitytool.vdf"))
                .unwrap()
                .contains("New tool")
        );
        assert_eq!(fs::read(&settings).unwrap(), b"custom settings");
        assert_eq!(fs::metadata(&settings).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(settings.parent().unwrap()).unwrap().mode() & 0o777,
            0o750
        );
        assert_eq!(fs::metadata(&tool).unwrap().mode() & 0o777, 0o750);
        assert_eq!(fs::read_link(tool.join("custom-helper")).unwrap(), outside);
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
        assert_eq!(
            fs::read_link(tool.join("relative-settings")).unwrap(),
            Path::new("settings/profile.json")
        );
        assert_eq!(
            fs::read_link(tool.join("broken-link")).unwrap(),
            Path::new("missing")
        );
        for name in ["nucleon-runner", "run"] {
            assert_eq!(fs::metadata(tool.join(name)).unwrap().mode() & 0o777, 0o755);
        }
        keyvalues_parser::parse(&fs::read_to_string(tool.join("toolmanifest.vdf")).unwrap())
            .unwrap();
        no_staging_directories(temp.path());
    }

    #[test]
    fn installed_runner_can_be_the_replacement_source() {
        let temp = tempdir().unwrap();
        let tool = installed_tool(temp.path());
        let runner = tool.join("nucleon-runner");
        let previous = fs::read(&runner).unwrap();
        write_tool_bundle(&tool, "nucleon-test", "Refreshed tool", &runner).unwrap();
        assert_eq!(fs::read(runner).unwrap(), previous);
        assert!(
            fs::read_to_string(tool.join("compatibilitytool.vdf"))
                .unwrap()
                .contains("Refreshed tool")
        );
        no_staging_directories(temp.path());
    }

    #[cfg(unix)]
    #[test]
    fn destination_files_and_symlinks_are_preserved_instead_of_replaced() {
        let temp = tempdir().unwrap();
        let tool = installed_tool(temp.path());
        let previous = bundle_contents(&tool);
        let link = temp.path().join("linked-tool");
        std::os::unix::fs::symlink(&tool, &link).unwrap();
        let file = temp.path().join("file");
        fs::write(&file, b"unrelated").unwrap();
        for target in [&file, &link] {
            assert!(
                write_tool_bundle(
                    target,
                    "nucleon-test",
                    "Replacement",
                    &temp.path().join("runner-source")
                )
                .is_err()
            );
        }
        assert_eq!(fs::read(file).unwrap(), b"unrelated");
        assert_eq!(fs::read_link(link).unwrap(), tool);
        assert_eq!(bundle_contents(&tool), previous);
        no_staging_directories(temp.path());
    }

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
