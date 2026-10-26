//! NGX policy on top of object's PE/export reader. No PE offsets are interpreted here.
use anyhow::{Context, Result, ensure};
use object::read::pe::ExportTarget;
use std::collections::BTreeMap;

const REQUIRED: &[&str] = &[
    "Init_Ext",
    "Init_ProjectID",
    "AllocateParameters",
    "GetParameters",
    "GetCapabilityParameters",
    "CreateFeature",
    "EvaluateFeature",
    "DestroyParameters",
    "ReleaseFeature",
];

/// A value is present for a named forwarder, absent for a local implementation.
pub(super) fn exports(data: &[u8]) -> Result<BTreeMap<String, Option<String>>> {
    ensure!(
        crate::runtime_checks::pe_abi(data)? == crate::runtime_checks::PeAbi::X64,
        "NGX bridge must be an ordinary x64 PE image"
    );
    let object::File::Pe64(image) = object::File::parse(data)? else {
        anyhow::bail!("NGX bridge must be PE32+");
    };
    let table = image
        .export_table()?
        .context("Bridge has no export table")?;
    let exports = table.exports()?;
    ensure!(exports.len() < 65536, "Invalid NGX export count");
    let mut names = BTreeMap::new();
    for export in exports {
        let Some(name) = export.name else { continue };
        if !name.starts_with(b"NVSDK_NGX_") {
            continue;
        }
        ensure!(
            name.len() < 512 && name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_'),
            "Invalid NGX export name"
        );
        let name = std::str::from_utf8(name)?.to_owned();
        let target = match export.target {
            ExportTarget::Address(_) => None,
            ExportTarget::ForwardByName(library, name) => Some(format!(
                "{}.{}",
                std::str::from_utf8(library)?,
                std::str::from_utf8(name)?
            )),
            ExportTarget::ForwardByOrdinal(_, _) => {
                anyhow::bail!("NGX ordinal forwarding is unsupported")
            }
        };
        ensure!(names.insert(name, target).is_none(), "Duplicate NGX export");
    }
    for suffix in REQUIRED {
        ensure!(
            names.contains_key(&format!("NVSDK_NGX_D3D12_{suffix}")),
            "GPTK bridge lacks required DX12 export {suffix}"
        );
    }
    Ok(names)
}

pub(super) fn verify_forwarder(
    data: &[u8],
    expected: &BTreeMap<String, Option<String>>,
) -> Result<()> {
    let actual = exports(data)?;
    ensure!(
        actual.len() == expected.len(),
        "Forwarder exports differ from vendor bridge"
    );
    for name in expected.keys() {
        ensure!(
            actual.get(name).and_then(Option::as_deref)
                == Some(format!("nvngx-on-metalfx.{name}").as_str()),
            "NGX export {name} does not forward to the original bridge"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    const FORWARDER: &[u8] = include_bytes!("fixtures/ngx-forward.bin");

    #[test]
    fn library_reads_complete_exports_and_verifies_forward_targets() {
        let names = exports(FORWARDER).unwrap();
        assert_eq!(names.len(), REQUIRED.len());
        verify_forwarder(FORWARDER, &names).unwrap();
        let mut wrong = FORWARDER.to_vec();
        let start = wrong
            .windows(b"nvngx-on-metalfx.".len())
            .position(|w| w == b"nvngx-on-metalfx.")
            .unwrap();
        wrong[start] = b'x';
        assert!(verify_forwarder(&wrong, &names).is_err());
        let mut missing = FORWARDER.to_vec();
        // Modify the named export and its forward target without changing table layout.
        for start in (0..missing.len() - b"Init_ProjectID".len())
            .filter(|i| FORWARDER[*i..].starts_with(b"Init_ProjectID"))
        {
            missing[start] = b'X';
        }
        assert!(exports(&missing).is_err());
        for length in [0, 2, 64, 128, 256, 512] {
            assert!(exports(&FORWARDER[..length]).is_err());
        }
    }

    proptest! {
        #[test]
        fn arbitrary_pe_data_never_panics(data in prop::collection::vec(any::<u8>(), 0..8192)) {
            let _ = exports(&data);
        }
        #[test]
        fn mutated_exports_never_bypass_name_or_forward_target_checks(
            edits in prop::collection::vec((0..FORWARDER.len(), any::<u8>()), 1..24)
        ) {
            let mut data = FORWARDER.to_vec();
            for (offset, byte) in edits { data[offset] = byte; }
            if let Ok(names) = exports(&data) {
                prop_assert!(REQUIRED.iter().all(|s| names.contains_key(&format!("NVSDK_NGX_D3D12_{s}"))), "Required exports must remain present");
                prop_assert!(names.keys().all(|n| n.len() < 512 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')));
            }
            if verify_forwarder(&data, &exports(FORWARDER).unwrap()).is_ok() {
                prop_assert!(exports(&data).unwrap() == exports(FORWARDER).unwrap());
            }
        }
    }
}
