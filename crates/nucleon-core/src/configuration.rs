//! Actionable errors for explicit launch configuration.
use anyhow::{Context, Result, bail};
use std::{fs, path::Path};

pub fn environment(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => bail!(
            "Invalid {name}: value is not valid UTF-8. Replace it with a valid value or remove {name} from Steam Launch Options."
        ),
    }
}

pub fn invalid(source: &str, value: &str, problem: &str, correction: &str) -> anyhow::Error {
    anyhow::anyhow!("Invalid {source}={value:?}: {problem}. {correction}")
}

pub fn engine(source: &str, value: &str) -> Result<crate::detector::TargetEngine> {
    crate::detector::TargetEngine::parse(value).ok_or_else(|| invalid(source, value,
        "unknown engine", "Choose auto, gptk, dxmt, dxvk, vkd3d, kosmickrisp, or wine; remove the override to use the default."))
}

pub fn boolean(source: &str, value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(invalid(
            source,
            value,
            "expected a boolean",
            "Use 1/0 or true/false, or remove the override.",
        )),
    }
}

/// A present configuration file is intentional, even if it is empty or unreadable.
pub fn optional_file(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Cannot read configuration {}. Fix its permissions/contents or remove it to use the default", path.display())),
    }
}

pub fn vulkan_manifest(source: &str, path: &Path) -> Result<()> {
    let context = || {
        format!(
            "Invalid {source}: Vulkan manifest {}. Select a readable ICD JSON file or remove this override",
            path.display()
        )
    };
    let bytes = fs::read(path).with_context(context)?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).with_context(context)?;
    let library = json["ICD"]["library_path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("{}: missing ICD.library_path", context()))?;
    let library = Path::new(library);
    let library = if library.is_absolute() {
        library.to_path_buf()
    } else {
        path.parent().unwrap_or(Path::new(".")).join(library)
    };
    if !library.is_file() {
        bail!(
            "{}: driver library {} is missing or is not a regular file",
            context(),
            library.display()
        );
    }
    fs::File::open(&library).with_context(|| {
        format!(
            "{}: driver library {} is missing or unreadable",
            context(),
            library.display()
        )
    })?;
    Ok(())
}

/// Validate the winning configured path before discovery can silently use another installation.
pub fn component_path(
    names: &[&str],
    saved: &Path,
    description: &str,
    reset: &str,
    valid: impl Fn(&Path) -> bool,
) -> Result<()> {
    for name in names {
        if let Some(value) = environment(name)? {
            if value.trim().is_empty() || !valid(Path::new(&value)) {
                return Err(invalid(
                    name,
                    &value,
                    description,
                    &format!(
                        "Point {name} to a valid installation or remove it from Steam Launch Options."
                    ),
                ));
            }
            return Ok(());
        }
    }
    if let Some(value) = optional_file(saved)? {
        let value = value.trim();
        if value.is_empty() || !valid(Path::new(value)) {
            return Err(invalid(
                &saved.display().to_string(),
                value,
                description,
                &format!("Correct this saved path or run '{reset}' to reset it."),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_values_name_the_source_and_offer_a_correction() {
        let error = engine("NUCLEON_ENGINE", "dxmv").unwrap_err().to_string();
        assert!(
            error.contains("NUCLEON_ENGINE")
                && error.contains("dxmv")
                && error.contains("Choose auto")
        );
        assert!(engine("engine", "automatic").is_ok());
        for value in ["1", "TRUE", "false", "0"] {
            assert!(boolean("NUCLEON_OVERLAY", value).is_ok());
        }
        assert!(
            boolean("NUCLEON_OVERLAY", "yes")
                .unwrap_err()
                .to_string()
                .contains("Use 1/0")
        );
    }
    #[test]
    fn vulkan_errors_identify_manifest_and_missing_library() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icd.json");
        fs::write(&path, "not json").unwrap();
        assert!(
            format!(
                "{:#}",
                vulkan_manifest("VK_DRIVER_FILES", &path).unwrap_err()
            )
            .contains("VK_DRIVER_FILES")
        );
        fs::write(&path, r#"{"ICD":{"library_path":"missing.dylib"}}"#).unwrap();
        assert!(
            format!(
                "{:#}",
                vulkan_manifest("profile.vulkan_icd", &path).unwrap_err()
            )
            .contains("missing.dylib")
        );
        fs::write(dir.path().join("missing.dylib"), "driver").unwrap();
        assert!(vulkan_manifest("profile.vulkan_icd", &path).is_ok());
    }
}
