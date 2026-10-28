//! Bounded appinfo framing around steam-vdf-parser's binary KeyValues decoder.
use anyhow::{Context, Result, bail, ensure};

const V40: u32 = 0x07564428;
const V41: u32 = 0x07564429;
const ENTRY_HEADER: usize = 68;
const MAX_ENTRY_SIZE: usize = 8 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .context("Truncated appinfo integer")?
            .try_into()?,
    ))
}

/// Missing apps or platform fields are unknown, not evidence of Windows-only support.
pub(super) fn read_app(data: &[u8], appid: u32) -> Result<Option<steam_vdf_parser::Vdf<'static>>> {
    let magic = u32_at(data, 0)?;
    let (header_size, apps_end, table) = match magic {
        V40 => (8, data.len(), None),
        V41 => {
            let offset = u64::from_le_bytes(
                data.get(8..16)
                    .context("Truncated appinfo string-table offset")?
                    .try_into()?,
            );
            let offset =
                usize::try_from(offset).context("Appinfo string-table offset is too large")?;
            ensure!(
                offset >= 16 && offset < data.len(),
                "Invalid appinfo string-table offset"
            );
            let table = &data[offset..];
            let count = u32_at(table, 0)? as usize;
            // Every string needs at least one byte. Bound the decoder's allocation before calling it.
            ensure!(
                count <= table.len() - 4,
                "Invalid appinfo string-table count"
            );
            let mut rest = &table[4..];
            for _ in 0..count {
                rest = after_c_string(rest)?;
            }
            ensure!(rest.is_empty(), "Trailing appinfo string-table data");
            (16, offset, Some(table))
        }
        _ => bail!("Unsupported appinfo format {magic:#010x}"),
    };
    ensure!(data.len() >= header_size, "Truncated appinfo header");
    let mut offset = header_size;
    let mut selected = None;
    let mut terminated = false;
    while offset < apps_end {
        let id = u32_at(&data[..apps_end], offset)?;
        if id == 0 {
            ensure!(offset + 4 == apps_end, "Trailing appinfo entry data");
            terminated = true;
            break;
        }
        let size = u32_at(&data[..apps_end], offset + 4)? as usize;
        ensure!(
            size >= ENTRY_HEADER - 8,
            "Appinfo entry {id} is smaller than its header"
        );
        let end = offset
            .checked_add(8)
            .and_then(|start| start.checked_add(size))
            .context("Appinfo entry size overflow")?;
        ensure!(
            end <= apps_end,
            "Appinfo entry {id} crosses the entries boundary"
        );
        if id == appid {
            ensure!(selected.is_none(), "Duplicate appinfo entry for {appid}");
            ensure!(size <= MAX_ENTRY_SIZE, "Appinfo entry {appid} is too large");
            validate_object(&data[offset + ENTRY_HEADER..end], table.is_some())?;
            selected = Some(&data[offset..end]);
        }
        offset = end;
    }
    ensure!(terminated, "Missing appinfo entry terminator");
    let Some(entry) = selected else {
        return Ok(None);
    };

    // Decode only this app. The crate checks for a full header before checking the zero-ID
    // terminator, so pad its input after the validated table/terminator to avoid rejecting
    // valid short tails. No padding is included in the selected app's declared size.
    let mut input = data[..header_size].to_vec();
    input.extend_from_slice(entry);
    input.extend_from_slice(&0u32.to_le_bytes());
    if let Some(table) = table {
        let table_offset = input.len() as u64;
        input[8..16].copy_from_slice(&table_offset.to_le_bytes());
        input.extend_from_slice(table);
    }
    input.extend_from_slice(&[0; ENTRY_HEADER]);
    let vdf = steam_vdf_parser::parse_appinfo(&input).context("Invalid appinfo KeyValues")?;
    Ok(Some(vdf.into_owned()))
}

#[cfg(test)]
fn macos_support(data: &[u8], appid: u32) -> Result<Option<bool>> {
    let Some(vdf) = read_app(data, appid)? else {
        return Ok(None);
    };
    let id = appid.to_string();
    let Some(oslist) = vdf.get_str(&[&id, "appinfo", "common", "oslist"]) else {
        return Ok(None);
    };
    let platforms = oslist.split(',').map(str::trim).collect::<Vec<_>>();
    if platforms.is_empty() || platforms.iter().any(|p| p.is_empty()) {
        return Ok(None);
    }
    Ok(Some(
        platforms.iter().any(|p| p.eq_ignore_ascii_case("macos")),
    ))
}

fn after_c_string(data: &[u8]) -> Result<&[u8]> {
    let end = memchr::memchr(0, data).context("Unterminated appinfo string")?;
    Ok(&data[end + 1..])
}

/// Iteratively check structure and recursion depth before invoking the recursive decoder.
/// The decoder accepts EOF as an object terminator; platform detection requires complete objects.
fn validate_object(mut data: &[u8], indexed_keys: bool) -> Result<()> {
    let mut depth = 1;
    while let Some((&kind, rest)) = data.split_first() {
        data = rest;
        if kind == 8 {
            depth -= 1;
            if depth == 0 {
                ensure!(data.is_empty(), "Trailing appinfo object data");
                return Ok(());
            }
            continue;
        }
        data = if indexed_keys {
            data.get(4..).context("Truncated appinfo key index")?
        } else {
            after_c_string(data)?
        };
        data = match kind {
            0 => {
                depth += 1;
                ensure!(depth <= MAX_DEPTH, "Appinfo nesting exceeds {MAX_DEPTH}");
                data
            }
            1 => after_c_string(data)?,
            2 | 3 | 4 | 6 => data.get(4..).context("Truncated appinfo value")?,
            7 => data.get(8..).context("Truncated appinfo value")?,
            5 => {
                let end = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .position(|unit| *unit == [0, 0])
                    .context("Unterminated appinfo wide string")?;
                &data[(end + 1) * 2..]
            }
            _ => bail!("Unknown appinfo value type {kind}"),
        };
    }
    bail!("Unterminated appinfo object")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [&str; 4] = ["appinfo", "common", "oslist", "description"];

    fn key(data: &mut Vec<u8>, kind: u8, name: &str, indexed: bool) {
        data.push(kind);
        if indexed {
            let index = KEYS.iter().position(|key| *key == name).unwrap() as u32;
            data.extend_from_slice(&index.to_le_bytes());
        } else {
            data.extend_from_slice(name.as_bytes());
            data.push(0);
        }
    }

    fn object(indexed: bool, oslist: Option<&str>) -> Vec<u8> {
        let mut body = Vec::new();
        key(&mut body, 0, "appinfo", indexed);
        key(&mut body, 0, "common", indexed);
        if let Some(oslist) = oslist {
            key(&mut body, 1, "oslist", indexed);
            body.extend_from_slice(oslist.as_bytes());
            body.push(0);
        }
        // Unrelated macos text must never be used as platform evidence.
        key(&mut body, 1, "description", indexed);
        body.extend_from_slice(b"macos legacy description\0");
        body.extend_from_slice(&[8, 8, 8]);
        body
    }

    fn fixture(indexed: bool, entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&(if indexed { V41 } else { V40 }).to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        if indexed {
            data.extend_from_slice(&0u64.to_le_bytes());
        }
        for (id, body) in entries {
            data.extend_from_slice(&id.to_le_bytes());
            data.extend_from_slice(&(60 + body.len() as u32).to_le_bytes());
            data.extend_from_slice(&[0; 60]);
            data.extend_from_slice(body);
        }
        data.extend_from_slice(&0u32.to_le_bytes());
        if indexed {
            let offset = data.len() as u64;
            data[8..16].copy_from_slice(&offset.to_le_bytes());
            data.extend_from_slice(&(KEYS.len() as u32).to_le_bytes());
            for name in KEYS {
                data.extend_from_slice(name.as_bytes());
                data.push(0);
            }
        }
        data
    }

    #[test]
    fn both_formats_use_only_the_matching_apps_platform_field() {
        for indexed in [false, true] {
            let mut data = fixture(
                indexed,
                &[
                    (1001, object(indexed, Some("windows,macos,linux"))),
                    (1002, object(indexed, Some("windows"))),
                    (1003, object(indexed, None)),
                    (1004, object(indexed, Some(" windows , MACOS "))),
                    (1005, object(indexed, Some("notmacos"))),
                    (1006, object(indexed, Some(""))),
                ],
            );
            // Embed the queried ID in another entry's metadata, before its real entry.
            let first = if indexed { 16 } else { 8 };
            data[first + 12..first + 16].copy_from_slice(&1002u32.to_le_bytes());
            assert_eq!(macos_support(&data, 1001).unwrap(), Some(true));
            assert_eq!(macos_support(&data, 1002).unwrap(), Some(false));
            assert_eq!(macos_support(&data, 1003).unwrap(), None);
            assert_eq!(macos_support(&data, 1004).unwrap(), Some(true));
            assert_eq!(macos_support(&data, 1005).unwrap(), Some(false));
            assert_eq!(macos_support(&data, 1006).unwrap(), None);
            assert_eq!(macos_support(&data, 9999).unwrap(), None);
        }
    }

    #[test]
    fn malformed_containers_return_errors_without_panicking() {
        for indexed in [false, true] {
            let data = fixture(indexed, &[(42, object(indexed, Some("macos")))]);
            for len in 0..data.len() {
                assert!(
                    macos_support(&data[..len], 42).is_err(),
                    "accepted truncation at {len}"
                );
            }
            let first = if indexed { 16 } else { 8 };
            for size in [0u32, 59, u32::MAX] {
                let mut invalid = data.clone();
                invalid[first + 4..first + 8].copy_from_slice(&size.to_le_bytes());
                assert!(macos_support(&invalid, 42).is_err());
            }
            let mut unknown = data.clone();
            unknown[0] = 0xff;
            assert!(macos_support(&unknown, 42).is_err());
            let duplicate = fixture(
                indexed,
                &[
                    (42, object(indexed, Some("macos"))),
                    (42, object(indexed, Some("windows"))),
                ],
            );
            assert!(macos_support(&duplicate, 42).is_err());
            if indexed {
                for offset in [0u64, 15, data.len() as u64, u64::MAX] {
                    let mut invalid = data.clone();
                    invalid[8..16].copy_from_slice(&offset.to_le_bytes());
                    assert!(macos_support(&invalid, 42).is_err());
                }
                let table = u64::from_le_bytes(data[8..16].try_into().unwrap()) as usize;
                let mut invalid = data.clone();
                invalid[table..table + 4].copy_from_slice(&u32::MAX.to_le_bytes());
                assert!(macos_support(&invalid, 42).is_err());
                let mut invalid = data.clone();
                invalid[first + ENTRY_HEADER + 1..first + ENTRY_HEADER + 5]
                    .copy_from_slice(&u32::MAX.to_le_bytes());
                assert!(macos_support(&invalid, 42).is_err());
            }
        }
    }

    #[test]
    fn malformed_or_excessively_nested_objects_are_rejected() {
        for indexed in [false, true] {
            let mut incomplete = object(indexed, Some("macos"));
            incomplete.pop();
            let mut trailing = object(indexed, Some("macos"));
            trailing.push(0);
            let mut deep = Vec::new();
            for _ in 0..MAX_DEPTH {
                key(&mut deep, 0, "appinfo", indexed);
            }
            deep.extend(std::iter::repeat_n(8, MAX_DEPTH + 1));
            for body in [incomplete, trailing, deep] {
                assert!(macos_support(&fixture(indexed, &[(42, body)]), 42).is_err());
            }
        }
    }
}
