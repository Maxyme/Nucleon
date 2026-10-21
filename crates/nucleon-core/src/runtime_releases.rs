//! Wine/FEX release consumption. The builder is a separate repository.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

const RELEASES: &str = "https://github.com/Maxyme/isotope/releases/download";
const MAX_MANIFEST: u64 = 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Asset {
    file: String,
    sha256: String,
}
#[derive(Debug, Deserialize)]
struct Release {
    schema: u32,
    version: String,
    component: String,
    binary: Asset,
}

fn version(value: &str) -> Result<&str> {
    let parsed =
        semver::Version::parse(value).context("Use an exact release version such as 0.1.0-rc.1")?;
    ensure!(
        parsed.build.is_empty(),
        "Release versions cannot contain build metadata"
    );
    Ok(value)
}

fn manifest(url: &str) -> Result<Release> {
    let mut file = crate::downloads::download(url, MAX_MANIFEST)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).context("Invalid Wine/FEX release metadata")
}

fn asset<'a>(release: &'a Release, expected_version: Option<&str>) -> Result<&'a Asset> {
    ensure!(
        release.schema == 2 && release.component == "isotope-core",
        "Unsupported Wine/FEX release metadata"
    );
    version(&release.version)?;
    ensure!(
        expected_version.is_none_or(|v| v == release.version),
        "Release version does not match the requested version"
    );
    let file = &release.binary.file;
    ensure!(
        !file.is_empty()
            && !file.contains(['/', '\\'])
            && file.ends_with(".tar.gz")
            && file
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
        "Invalid release asset name"
    );
    checksum(&release.binary.sha256)?;
    Ok(&release.binary)
}

fn checksum(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "Expected a 64-character SHA256 checksum"
    );
    Ok(())
}

pub fn install(
    source: &str,
    requested_version: Option<&str>,
    sha256: Option<&str>,
) -> Result<PathBuf> {
    if let Some(sha) = sha256 {
        checksum(sha)?;
    }
    if matches!(source, "isotope" | "wine-fex") {
        let requested = version(
            requested_version.context("isotope requires --version <exact-release-version>")?,
        )?;
        let base = format!("{RELEASES}/v{requested}");
        let release = manifest(&format!("{base}/runtime-release.json"))?;
        let binary = asset(&release, Some(requested))?;
        if let Some(sha) = sha256 {
            ensure!(
                sha.eq_ignore_ascii_case(&binary.sha256),
                "Provided checksum differs from release metadata"
            );
        }
        return crate::managed_runtime::install_url(
            &format!("{base}/{}", binary.file),
            &binary.sha256,
        );
    }
    ensure!(
        requested_version.is_none(),
        "--version is only used with the named isotope release; paths and URLs select their own artifact"
    );
    ensure!(
        !source.starts_with("http://"),
        "Runtime downloads require HTTPS"
    );
    if source.starts_with("https://") {
        let sha = match sha256 {
            Some(sha) => sha.to_owned(),
            None => {
                let (base, name) = source.rsplit_once('/').context("Invalid archive URL")?;
                ensure!(
                    !name.contains(['?', '#']),
                    "Use --sha256 for URLs with queries or fragments"
                );
                let release = manifest(&format!("{base}/runtime-release.json")).context(
                    "Provide --sha256 for archive URLs without adjacent runtime-release.json",
                )?;
                let binary = asset(&release, None)?;
                ensure!(
                    binary.file == name,
                    "URL does not match the release binary asset"
                );
                binary.sha256.clone()
            }
        };
        return crate::managed_runtime::install_url(source, &sha);
    }
    let path = Path::new(source);
    ensure!(path.exists(), "Runtime path does not exist: {source}");
    if path.is_dir() {
        ensure!(sha256.is_none(), "--sha256 applies to archives");
        return crate::managed_runtime::install(path);
    }
    // A local file is an explicit user-selected input; a supplied checksum is still enforced.
    let digest = sha256
        .map(str::to_owned)
        .map(Ok)
        .unwrap_or_else(|| crate::artifacts::digest(path))?;
    crate::managed_runtime::install_archive(path, &digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn releases_are_exact_and_assets_cannot_escape_the_release() {
        let mut release: Release = serde_json::from_value(serde_json::json!({"schema":2,"version":"0.1.0-rc.1","component":"isotope-core","binary":{"file":"isotope-0.1.0-rc.1-macos-arm64.tar.gz","sha256":"a".repeat(64)}})).unwrap();
        assert!(asset(&release, Some("0.1.0-rc.1")).is_ok());
        assert!(asset(&release, Some("0.1.0-rc.2")).is_err());
        release.binary.file = "../other.tar.gz".into();
        assert!(asset(&release, None).is_err());
        for value in ["latest", "../0.1.0", "0.1.0?token", "0.1.0/other"] {
            assert!(version(value).is_err());
        }
        assert!(install("isotope", None, None).is_err());
        assert!(install("wine-fex", None, None).is_err());
        assert!(install("/missing/runtime", Some("0.1.0"), None).is_err());
        assert!(install("http://example.invalid/file.tar.gz", None, None).is_err());
    }
}
