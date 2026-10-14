#![forbid(unsafe_code)]

//! Exact-client contracts. ABI data comes from
//! the installed Valve image.
use crate::{client_image, fs_util::atomic_write_file, paths};
use anyhow::{Context, Result, bail, ensure};
use object::{Object, ObjectSection, ObjectSegment, read::macho::MachHeader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const EMBEDDED_CONTRACTS: &[&str] = &[
    include_str!("../../../assets/steam-contracts/macos-arm64/client-1788652215.toml"),
    include_str!("../../../assets/steam-contracts/macos-arm64/client-1791918634.toml"),
];

fn reviewed_contract(image: &[u8]) -> Result<ClientContract> {
    let slice = client_image::extract_arm64_slice(image)?;
    let file = object::File::parse(slice)?;
    let uuid = file.mach_uuid()?.context("Steam image has no UUID")?;
    let uuid: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    for embedded in EMBEDDED_CONTRACTS {
        let contract: ClientContract = toml::from_str(embedded)?;
        if contract.client.uuid == uuid {
            return Ok(contract);
        }
    }
    bail!("Unreviewed Steam image UUID: {uuid}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientContract {
    pub format: String,
    pub revision: u32,
    pub client: ClientIdentity,
    pub provenance: Provenance,
    pub eligibility: Eligibility,
    pub patches: Vec<InstructionSite>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientIdentity {
    pub platform: String,
    pub architecture: String,
    pub steam_build: u64,
    pub uuid: String,
    pub text_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub method: String,
    pub source: String,
    pub limitations: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Eligibility {
    pub virtual_address: u64,
    pub entry_words: Vec<u32>,
    pub abi: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatchKind {
    PlatformStateRead,
    OsListToolGate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstructionSite {
    pub kind: PatchKind,
    pub virtual_address: u64,
    pub expected_word: u32,
    pub evidence: String,
}

impl InstructionSite {
    pub fn replacement_word(&self) -> Result<u32> {
        match self.kind {
            PatchKind::PlatformStateRead => {
                // AArch64 unsigned-immediate LDRB to a W register. Keep its
                // destination register, replacing the native platform-state
                // read with MOVZ Wd,#1. No Rust pointer into a C++ object exists.
                ensure!(
                    self.expected_word & 0xffc0_0000 == 0x3940_0000,
                    "Platform-state site is not an unsigned-immediate LDRB"
                );
                Ok(0x5280_0020 | (self.expected_word & 31))
            }
            PatchKind::OsListToolGate => {
                ensure!(
                    self.expected_word & 0xff00_001f == 0xb400_0000,
                    "OS-list gate is not CBZ X0"
                );
                Ok(0xd503_201f)
            }
        }
    }
}

pub struct ValidatedContract {
    contract: ClientContract,
    text_address: u64,
    text_size: usize,
    preferred_base: u64,
}

pub fn text_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_text(contract: &ClientContract, address: u64, text: &[u8]) -> Result<()> {
    ensure!(
        text_digest(text) == contract.client.text_sha256,
        "Steam executable text does not match the reviewed contract"
    );
    let word_at = |va: u64| -> Result<u32> {
        ensure!(va & 3 == 0, "Unaligned AArch64 site");
        let offset = usize::try_from(va.checked_sub(address).context("Site before text")?)?;
        let end = offset.checked_add(4).context("Site range overflow")?;
        let bytes = text
            .get(offset..end)
            .context("Site outside executable text")?;
        Ok(u32::from_le_bytes(bytes.try_into()?))
    };
    ensure!(
        contract.eligibility.abi == "c-self-u32-bool",
        "Unsupported callback ABI"
    );
    ensure!(
        contract.eligibility.entry_words.len() >= 4,
        "Missing function-entry witness"
    );
    for (i, expected) in contract.eligibility.entry_words.iter().enumerate() {
        let va = contract
            .eligibility
            .virtual_address
            .checked_add((i as u64) * 4)
            .context("Entry range overflow")?;
        ensure!(
            word_at(va)? == *expected,
            "Eligibility entry witness mismatch"
        );
    }
    ensure!(!contract.patches.is_empty(), "Empty contract patch plan");
    let mut addresses = HashSet::new();
    for site in &contract.patches {
        ensure!(
            addresses.insert(site.virtual_address),
            "Duplicate patch site"
        );
        ensure!(!site.evidence.is_empty(), "Missing site evidence");
        ensure!(
            word_at(site.virtual_address)? == site.expected_word,
            "Instruction witness mismatch at {:#x}",
            site.virtual_address
        );
        site.replacement_word()?;
    }
    Ok(())
}

/// Validate an owned file image; code signing changes outside __text are allowed.
fn validate_image(contract: ClientContract, image: &[u8]) -> Result<ValidatedContract> {
    ensure!(
        contract.format == "nucleon-steam-contract" && contract.revision == 1,
        "Unsupported Steam contract format/revision"
    );
    ensure!(
        contract.client.platform == "macos" && contract.client.architecture == "arm64",
        "Unsupported Steam contract platform/architecture"
    );
    let slice = client_image::extract_arm64_slice(image)?;
    let file = object::File::parse(slice)?;
    let uuid = file.mach_uuid()?.context("Steam image has no UUID")?;
    let actual_uuid: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    ensure!(
        actual_uuid == contract.client.uuid,
        "Unreviewed Steam image UUID: {actual_uuid}"
    );
    let preferred_base = file
        .segments()
        .find(|segment| segment.name().ok().flatten() == Some("__TEXT"))
        .context("No __TEXT segment")?
        .address();
    let section = file
        .section_by_name("__text")
        .context("No executable text section")?;
    let text = section.data()?;
    validate_text(&contract, section.address(), text)?;
    Ok(ValidatedContract {
        contract,
        text_address: section.address(),
        text_size: text.len(),
        preferred_base,
    })
}

impl ValidatedContract {
    pub fn contract(&self) -> &ClientContract {
        &self.contract
    }
    pub fn text_address(&self) -> u64 {
        self.text_address
    }
    pub fn text_size(&self) -> usize {
        self.text_size
    }
    pub fn preferred_base(&self) -> u64 {
        self.preferred_base
    }
    pub fn validate_loaded_header(&self, snapshot: &[u8]) -> Result<()> {
        // A loaded header snapshot contains load commands, not the on-disk symbol table.
        // Parsing a whole Object file here would incorrectly require __LINKEDIT bytes.
        let header = object::macho::MachHeader64::<object::LittleEndian>::parse(snapshot, 0)?;
        let endian = object::LittleEndian;
        ensure!(
            header.is_little_endian() && header.cputype(endian) == object::macho::CPU_TYPE_ARM64,
            "Loaded Steam image is not ARM64"
        );
        let uuid = header
            .uuid(endian, snapshot, 0)?
            .context("Loaded image has no UUID")?;
        let uuid: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
        ensure!(
            uuid == self.contract.client.uuid,
            "Loaded image UUID mismatch"
        );
        Ok(())
    }

    /// Snapshot supplied by the hook through the kernel's memory-copy API.
    pub fn validate_loaded_text(&self, snapshot: &[u8]) -> Result<()> {
        ensure!(
            snapshot.len() == self.text_size,
            "Incomplete loaded-image snapshot"
        );
        validate_text(&self.contract, self.text_address, snapshot)
    }
}

pub fn cache_path(contract: &ClientContract) -> PathBuf {
    paths::steam_contracts_dir().join(format!("client-{}.toml", contract.client.uuid))
}

/// Every invocation revalidates the source image.
pub fn ensure_contract_for_image(path: &Path) -> Result<(PathBuf, ValidatedContract)> {
    let validated = inspect_contract_image(path)?;
    let cache = cache_path(&validated.contract);
    let payload = toml::to_string_pretty(&validated.contract)?;
    if fs::read_to_string(&cache).ok().as_deref() != Some(&payload) {
        fs::create_dir_all(paths::steam_contracts_dir())?;
        atomic_write_file(&cache, payload)?;
    }
    Ok((cache, validated))
}

/// Read-only inspection: no cache writes, native hooks, or Steam changes.
pub fn inspect_contract_image(path: &Path) -> Result<ValidatedContract> {
    let image = fs::read(path).with_context(|| format!("Read {}", path.display()))?;
    validate_image(reviewed_contract(&image)?, &image)
}

pub fn ensure_contract_for_installed_steam() -> Result<(PathBuf, ValidatedContract)> {
    let candidates = [
        paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS/steamclient.dylib"),
        paths::steam_app().join("Contents/MacOS/steamclient.dylib"),
    ];
    let Some(path) = candidates.iter().find(|path| path.is_file()) else {
        bail!("steamclient.dylib not found for contract validation");
    };
    ensure_contract_for_image(path)
}

pub fn has_current_cached_contract() -> bool {
    let path =
        paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS/steamclient.dylib");
    inspect_contract_image(&path).is_ok_and(|reviewed| cache_path(reviewed.contract()).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_contracts_have_unique_identities_and_valid_instruction_kinds() {
        let mut uuids = HashSet::new();
        for embedded in EMBEDDED_CONTRACTS {
            let contract: ClientContract = toml::from_str(embedded).unwrap();
            assert!(uuids.insert(contract.client.uuid));
            assert!(!contract.patches.is_empty());
            for site in contract.patches {
                site.replacement_word().unwrap();
            }
        }
    }

    #[test]
    fn loaded_header_needs_no_symbol_table_payload() {
        let contract: ClientContract = toml::from_str(EMBEDDED_CONTRACTS[0]).unwrap();
        let uuid: Vec<u8> = (0..32)
            .step_by(2)
            .map(|offset| {
                u8::from_str_radix(&contract.client.uuid[offset..offset + 2], 16).unwrap()
            })
            .collect();
        let mut header = Vec::new();
        for word in [
            0xfeed_facf_u32,
            object::macho::CPU_TYPE_ARM64,
            0,
            6,
            2,
            48,
            0,
            0,
            object::macho::LC_UUID,
            24,
        ] {
            header.extend_from_slice(&word.to_le_bytes());
        }
        header.extend_from_slice(&uuid);
        // A real load command references symbol/string data outside this header-only snapshot.
        for word in [object::macho::LC_SYMTAB, 24, 4096, 1, 8192, 64] {
            header.extend_from_slice(&word.to_le_bytes());
        }
        assert!(object::File::parse(header.as_slice()).is_err());
        let reviewed = ValidatedContract {
            contract,
            text_address: 0,
            text_size: 0,
            preferred_base: 0,
        };
        reviewed.validate_loaded_header(&header).unwrap();
        assert!(reviewed.validate_loaded_header(&header[..32]).is_err());
        header[40] ^= 1;
        assert!(reviewed.validate_loaded_header(&header).is_err());
    }

    #[test]
    #[ignore = "requires a local reviewed Steam client"]
    fn reviewed_client_rejects_modified_text_and_unknown_uuid() {
        let path = std::env::var("NUCLEON_TEST_STEAM_CLIENT").unwrap();
        let image = fs::read(path).unwrap();
        let contract = reviewed_contract(&image).unwrap();
        let reviewed = validate_image(contract.clone(), &image).unwrap();
        let arm64 = client_image::extract_arm64_slice(&image).unwrap();
        let commands = u32::from_le_bytes(arm64[20..24].try_into().unwrap()) as usize;
        let mut header = arm64[..32 + commands].to_vec();
        reviewed.validate_loaded_header(&header).unwrap();
        assert!(reviewed.validate_loaded_header(&header[..32]).is_err());
        header[4] ^= 1;
        assert!(reviewed.validate_loaded_header(&header).is_err());
        let mut slice = client_image::extract_arm64_slice(&image).unwrap().to_vec();
        let file = object::File::parse(slice.as_slice()).unwrap();
        let offset = file
            .section_by_name("__text")
            .unwrap()
            .file_range()
            .unwrap()
            .0 as usize;
        let uuid = file.mach_uuid().unwrap().unwrap();
        slice[offset] ^= 1;
        assert!(validate_image(contract, &slice).is_err());
        // LC_UUID contains the selected identity once in the file header.
        let uuid_offset = image.windows(16).position(|bytes| bytes == uuid).unwrap();
        let mut unknown = image;
        unknown[uuid_offset] ^= 1;
        assert!(reviewed_contract(&unknown).is_err());
    }
}
