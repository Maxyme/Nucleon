//! Actionable errors for explicit launch configuration.
use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Path, PathBuf},
};

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

/// Decode one complete JSON document, preserving the field path and parser location.
pub(crate) fn json<T: serde::de::DeserializeOwned>(text: &str, path: &Path) -> Result<T> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = serde_path_to_error::deserialize(&mut deserializer)
        .map_err(|error| parse_error(error, path, "JSON"))?;
    // Deserializing a value alone does not reject a second value or trailing garbage.
    deserializer.end().with_context(|| {
        format!(
            "Invalid JSON configuration {} after the document",
            path.display()
        )
    })?;
    Ok(value)
}

pub(crate) fn optional_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    optional_file(path)?
        .map(|text| json(&text, path))
        .transpose()
}

pub(crate) fn toml<T: serde::de::DeserializeOwned>(text: &str, path: &Path) -> Result<T> {
    serde_path_to_error::deserialize(toml::Deserializer::new(text))
        .map_err(|error| parse_error(error, path, "TOML"))
}

fn parse_error<E: std::error::Error + Send + Sync + 'static>(
    error: serde_path_to_error::Error<E>,
    path: &Path,
    format: &str,
) -> anyhow::Error {
    let field = error.path().to_string();
    let location = if field.is_empty() || field == "." {
        "at document root".to_string()
    } else {
        format!("at field `{field}`")
    };
    anyhow::Error::new(error.into_inner()).context(format!(
        "Invalid {format} configuration {} {location}",
        path.display()
    ))
}

#[derive(serde::Deserialize)]
struct VulkanManifest {
    #[serde(rename = "ICD")]
    icd: VulkanIcd,
}

#[derive(serde::Deserialize)]
struct VulkanIcd {
    library_path: String,
    api_version: Option<String>,
}

pub(crate) struct ResolvedVulkanIcd {
    /// Retained so bundled runtimes can forbid absolute declarations.
    pub declared_library: PathBuf,
    pub library: PathBuf,
    api_version: Option<String>,
}

impl ResolvedVulkanIcd {
    /// Invalid or absent versions do not establish Vulkan compatibility.
    pub fn api_version(&self) -> Option<(u32, u32, u32)> {
        let mut parts = self.api_version.as_deref()?.split('.');
        let mut number = || {
            let part = parts.next()?;
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse::<u32>().ok())?
        };
        let version = (number()?, number()?, number()?);
        parts.next().is_none().then_some(version)
    }
}

/// Resolve a driver beside its manifest; package containment is the caller's policy.
pub(crate) fn resolve_vulkan_icd(path: &Path) -> Result<ResolvedVulkanIcd> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("Read Vulkan ICD manifest {}", path.display()))?;
    let manifest: VulkanManifest = json(&text, path)?;
    anyhow::ensure!(
        !manifest.icd.library_path.is_empty(),
        "Vulkan ICD manifest {}: empty ICD.library_path",
        path.display()
    );
    let declared_library = PathBuf::from(manifest.icd.library_path);
    let library = if declared_library.is_absolute() {
        declared_library.clone()
    } else {
        path.parent()
            .unwrap_or(Path::new("."))
            .join(&declared_library)
    };
    let library = library.canonicalize().with_context(|| {
        format!(
            "Vulkan ICD manifest {}: cannot resolve driver library {}",
            path.display(),
            library.display()
        )
    })?;
    anyhow::ensure!(
        library.is_file(),
        "Vulkan ICD manifest {}: driver library {} is not a regular file",
        path.display(),
        library.display()
    );
    Ok(ResolvedVulkanIcd {
        declared_library,
        library,
        api_version: manifest.icd.api_version,
    })
}

pub fn vulkan_manifest(source: &str, path: &Path) -> Result<()> {
    let context = || {
        format!(
            "Invalid {source}: Vulkan manifest {}. Select a readable ICD JSON file or remove this override",
            path.display()
        )
    };
    let icd = resolve_vulkan_icd(path).with_context(context)?;
    fs::File::open(&icd.library).with_context(|| {
        format!(
            "{}: driver library {} is missing or unreadable",
            context(),
            icd.library.display()
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
    fn typed_documents_report_nested_fields_and_original_parser_locations() {
        type Profiles = std::collections::BTreeMap<String, Vec<crate::profile::GameProfile>>;
        let path = Path::new("/configuration/profiles.json");
        let error = json::<Profiles>(r#"{"games":[{"hud":"yes"}]}"#, path).unwrap_err();
        let report = format!("{error:#}");
        for expected in [
            "/configuration/profiles.json",
            "games[0].hud",
            "boolean",
            "line 1",
            "column",
        ] {
            assert!(report.contains(expected), "{report}");
        }
        let error = toml::<Profiles>(
            "[[games]]\nhud = \"yes\"\n",
            Path::new("/configuration/profiles.toml"),
        )
        .unwrap_err();
        let report = format!("{error:#}");
        for expected in [
            "/configuration/profiles.toml",
            "games[0].hud",
            "boolean",
            "line 2",
        ] {
            assert!(report.contains(expected), "{report}");
        }
    }

    #[test]
    fn optional_json_keeps_missing_files_optional_and_rejects_invalid_complete_documents() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("profile.json");
        let load = || optional_json::<crate::profile::GameProfile>(&path);
        assert!(load().unwrap().is_none());
        for contents in [
            "",
            "{",
            "{} {}",
            "{} garbage",
            r#"{"hdu":true}"#,
            r#"{"hud":"yes"}"#,
        ] {
            fs::write(&path, contents).unwrap();
            let error = load().unwrap_err();
            assert!(format!("{error:#}").contains(&path.display().to_string()));
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
        fs::write(&path, "{\"hud\":false} \n\t").unwrap();
        assert_eq!(load().unwrap().unwrap().hud, Some(false));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(load().is_err());
    }

    #[test]
    fn vulkan_schema_reports_exact_library_field_and_accepts_extension_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("icd.json");
        fs::write(&path, r#"{"ICD":{"library_path":42}}"#).unwrap();
        let error = vulkan_manifest("VK_DRIVER_FILES", &path).unwrap_err();
        let report = format!("{error:#}");
        for expected in [
            "VK_DRIVER_FILES",
            "ICD.library_path",
            "expected a string",
            "line 1",
        ] {
            assert!(report.contains(expected), "{report}");
        }
        fs::write(temp.path().join("driver.dylib"), b"fixture").unwrap();
        fs::write(&path, r#"{"file_format_version":"1.0.0","ICD":{"library_path":"driver.dylib","api_version":"1.4.0"}}"#).unwrap();
        vulkan_manifest("VK_DRIVER_FILES", &path).unwrap();
    }

    #[test]
    fn vulkan_resolution_handles_absolute_relative_and_bare_manifest_paths() {
        let dir = tempfile::tempdir().unwrap();
        let driver = dir.path().join("driver.dylib");
        fs::write(&driver, b"fixture").unwrap();
        let path = dir.path().join("icd.json");
        for library in [Path::new("driver.dylib"), driver.as_path()] {
            fs::write(
                &path,
                serde_json::json!({"ICD":{"library_path":library}}).to_string(),
            )
            .unwrap();
            let resolved = resolve_vulkan_icd(&path).unwrap();
            assert_eq!(resolved.declared_library, library);
            assert_eq!(resolved.library, driver.canonicalize().unwrap());
            assert_eq!(resolved.api_version(), None);
        }

        // Use unique files in the current directory without changing global cwd.
        let driver = tempfile::NamedTempFile::new_in(".").unwrap();
        let manifest = tempfile::NamedTempFile::new_in(".").unwrap();
        fs::write(
            manifest.path(),
            serde_json::json!({"ICD":{"library_path":driver.path().file_name().unwrap().to_str().unwrap()}})
                .to_string(),
        )
        .unwrap();
        let bare = Path::new(manifest.path().file_name().unwrap());
        assert_eq!(
            resolve_vulkan_icd(bare).unwrap().library,
            driver.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn vulkan_resolution_rejects_invalid_documents_and_nonfile_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icd.json");
        fs::write(dir.path().join("driver.dylib"), b"fixture").unwrap();
        for (document, expected) in [
            (r#"{}"#, "ICD"),
            (r#"{"ICD":{}}"#, "library_path"),
            (r#"{"ICD":{"library_path":""}}"#, "ICD.library_path"),
            (r#"{"ICD":{"library_path":42}}"#, "ICD.library_path"),
            (
                r#"{"ICD":{"library_path":"driver.dylib","api_version":42}}"#,
                "ICD.api_version",
            ),
            (
                r#"{"ICD":{"library_path":"driver.dylib"}} {}"#,
                "after the document",
            ),
            (
                r#"{"ICD":{"library_path":"missing.dylib"}}"#,
                "missing.dylib",
            ),
            (r#"{"ICD":{"library_path":"."}}"#, "not a regular file"),
        ] {
            fs::write(&path, document).unwrap();
            let error = resolve_vulkan_icd(&path).err().unwrap();
            let report = format!("{error:#}");
            assert!(report.contains(expected), "{report}");
            assert!(report.contains(&path.display().to_string()), "{report}");
        }
        fs::remove_file(&path).unwrap();
        let report = format!("{:#}", resolve_vulkan_icd(&path).err().unwrap());
        assert!(report.contains("Read Vulkan ICD manifest"), "{report}");
    }

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
