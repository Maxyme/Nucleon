//! Puts Apple's Game Porting Toolkit Windows binaries onto the Unix-call ABI of the Wine build that
//! will host them.
//!
//! # Why this exists
//!
//! The D3DMetal binaries shipped inside GPTK (`dxgi.dll`, `d3d11.dll`, …) reach their Metal
//! implementation through one thunk that binds them to the host's Unix-call entry point:
//!
//! ```text
//! mov   r8d, <size>
//! call  [rip -> ntdll!NtQueryVirtualMemory]   ; locate our own image
//! test  eax, eax
//! jne   <fail>
//! mov   rax, [rsi]                            ; <- rewritten with the symbol, see below
//! mov   [rip + unix library handle], rax      ; used by every later Unix call
//! xor   eax, eax
//! ret / jmp <done>
//! fail: mov eax, 0xC0000135                   ; STATUS_DLL_NOT_FOUND
//! ```
//!
//! That thunk resolves the host entry point *by name*, and Wine has shipped two spellings of it with
//! slightly different conventions:
//!
//! * `__wine_unix_call_dispatcher` — what upstream Wine exports today, and what GPTK 4's binaries ask
//!   for out of the box. The thunk stores the value read out of the query buffer.
//! * `__wine_unix_call` — what Apple's older Game Porting Toolkit base Wine implements. It wants the
//!   address of the storage instead, so alongside the name the thunk's `movq (%rsi),%rax` has to
//!   become `movq %rsi,%rax`.
//!
//! Which pair is correct depends on which runtime ends up hosting the binaries: staging GPTK 4 onto
//! Apple's own base Wine needs the rewrite, staging it onto a current upstream Wine must leave the
//! binaries untouched. Renaming to a spelling the host does not export is not a graceful failure —
//! the thunk takes its `STATUS_DLL_NOT_FOUND` path on the first rendered frame, which looks like a
//! broken GPU driver rather than an ABI mismatch. So the direction is derived from the runtime and
//! never assumed, and a combination that cannot be bridged is refused instead of guessed at.
//!
//! # Why the opcode rewrite is signature-gated
//!
//! The opcode pattern is *common*: `movq (%rsi),%rax` is how a compiler loads a vtable pointer out of
//! `this`. In GPTK 4's own binaries that encoding appears 35 times in `nvapi64.dll` and 7 times in
//! `nvngx-on-metalfx.dll`, but only **one** occurrence per file is the init thunk. Rewriting every
//! occurrence turns each of those vtable loads into a call through an arbitrary object field: NVAPI
//! and NVNGX (DLSS) then fault inside an otherwise "working" GPTK runner. The opcode is therefore
//! rewritten only where the surrounding bytes identify the init thunk, in an executable section.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

/// Host entry point the Windows-side binaries ask for: current upstream Wine's spelling.
const DISPATCHER_SYMBOL: &[u8] = b"__wine_unix_call_dispatcher";
/// The same entry point as spelled by Apple's older GPTK base Wine.
const PLAIN_SYMBOL: &[u8] = b"__wine_unix_call";

/// `movq (%rsi), %rax` — reads the library handle out of the buffer filled by the query above.
const DEREFERENCE_RSI: [u8; 3] = [0x48, 0x8b, 0x06];
/// `movq %rsi, %rax` — passes the storage address, which is what Apple's base Wine expects.
const PASS_RSI_BY_VALUE: [u8; 3] = [0x48, 0x89, 0xf0];
/// `movq %rax, %rip+disp32` — stores into the DLL's Unix-library handle global.
const STORE_TO_HANDLE_GLOBAL: [u8; 3] = [0x48, 0x89, 0x05];
/// `mov eax, 0xC0000135` (`STATUS_DLL_NOT_FOUND`) — the thunk's own failure path, sitting a handful of
/// bytes past the store. It is what distinguishes the init thunk from an unrelated vtable load.
const STATUS_DLL_NOT_FOUND: [u8; 5] = [0xb8, 0x35, 0x01, 0x00, 0xc0];
/// How far past the store to look for [`STATUS_DLL_NOT_FOUND`].
const FAILURE_PATH_WINDOW: usize = 24;

/// Bumped whenever the staging rules change, so runners produced by older rules are rebuilt.
pub const ABI_REVISION: u32 = 2;

/// Runtime images whose exported Unix-call symbol determines the ABI used by GPTK's DLLs.
///
/// Kept public so runner source tracking fingerprints the exact inputs that ABI detection reads.
/// Adding a candidate here therefore invalidates a previously assembled runner instead of letting
/// detection and invalidation silently drift apart.
pub const WINE_UNIX_CALL_ABI_CANDIDATES: [&str; 5] = [
    "lib/wine/x86_64-windows/ntdll.dll",
    "lib64/wine/x86_64-windows/ntdll.dll",
    "lib/wine/i386-windows/ntdll.dll",
    // Some builds' ntdll predates the thunk; these images use it and name it identically.
    "lib/wine/x86_64-windows/win32u.dll",
    "lib/wine/x86_64-windows/ws2_32.dll",
];

/// The Unix-call ABI a Wine runtime implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WineUnixCallAbi {
    /// Upstream Wine: exports `__wine_unix_call_dispatcher`.
    Dispatcher,
    /// Apple's GPTK base Wine: exports plain `__wine_unix_call`.
    Plain,
}

impl std::fmt::Display for WineUnixCallAbi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dispatcher => f.write_str("__wine_unix_call_dispatcher"),
            Self::Plain => f.write_str("__wine_unix_call"),
        }
    }
}

/// Detects the Unix-call ABI implemented by the Wine runtime rooted at `wine_root`.
///
/// The authoritative evidence is the runtime's own Windows images: they bind to the entry point by name
/// in exactly the way GPTK's do, so whichever spelling this build's `ntdll.dll` carries is the spelling
/// it answers to. Probing the Unix-side libraries does not answer it — Apple's base Wine keeps both
/// names out of its `ntdll.so`, while upstream Wine mentions them for unrelated reasons.
///
/// Returns `None` when nothing readable names either entry point; callers must then leave binaries
/// untouched rather than pick a direction.
pub fn detect_wine_unix_call_abi(wine_root: &Path) -> Option<WineUnixCallAbi> {
    for relative in WINE_UNIX_CALL_ABI_CANDIDATES {
        let Ok(bytes) = fs::read(wine_root.join(relative)) else {
            continue;
        };
        if !find_symbols(&bytes, DISPATCHER_SYMBOL).is_empty() {
            return Some(WineUnixCallAbi::Dispatcher);
        }
        if !find_symbols(&bytes, PLAIN_SYMBOL).is_empty() {
            return Some(WineUnixCallAbi::Plain);
        }
    }
    None
}

/// True when `byte` could continue an identifier, i.e. when a match touching it is part of a longer
/// symbol name.
fn is_symbol_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Finds occurrences of `name` as a complete symbol rather than the prefix of a longer one.
///
/// Essential here: `__wine_unix_call` is a prefix of `__wine_unix_call_dispatcher`, so naive matching
/// reports every dispatcher reference as though it also named the plain entry point.
fn find_symbols(data: &[u8], name: &[u8]) -> Vec<usize> {
    find_in_ranges(data, name, |offset| {
        let before_clear = offset == 0 || !is_symbol_byte(data[offset - 1]);
        let after_clear = data
            .get(offset + name.len())
            .is_none_or(|byte| !is_symbol_byte(*byte));
        before_clear && after_clear
    })
}

/// A region of a PE image that is mapped into memory when the image is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MappedRegion {
    range: std::ops::Range<usize>,
    executable: bool,
}

/// What Nucleon rewrote inside one image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageBridge {
    /// Renamed copies of the host entry point found in mapped sections.
    pub symbol_rewrites: usize,
    /// Init-thunk dereferences rewritten to pass the storage address.
    pub init_sites_patched: usize,
    /// Copies of the name outside any section (symbol/debug tables). Left alone deliberately: nothing
    /// loads them, and editing them inflates the diff against Apple's binaries.
    pub unreferenced_symbol_copies: usize,
}

/// Summary of staging a GPTK Windows binary tree.
#[derive(Debug, Clone, Default)]
pub struct StageReport {
    /// Images rewritten while being staged, with per-image counts.
    pub bridged: Vec<(String, ImageBridge)>,
    /// Files copied through without modification.
    pub passthrough: usize,
}

impl StageReport {
    pub fn bridged_count(&self) -> usize {
        self.bridged.len()
    }
}

/// Reads a little-endian `u16`, yielding `None` instead of panicking on a truncated image.
/// Every header field below is read this way.
fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

/// Locates the file ranges a PE image maps into memory, plus whether they are executable.
///
/// Written by hand rather than through an object-file library because it needs exactly two things —
/// mapped byte ranges and execute permission — and must stay correct for images produced by Apple's
/// linker without inheriting a parser's opinion of what counts as valid. Every offset is bounds
/// checked; anything questionable yields `None`, which callers treat as "cannot rewrite".
fn mapped_regions(data: &[u8]) -> Option<Vec<MappedRegion>> {
    let signature = u32_at(data, 0x3c)? as usize;
    if data.get(signature..signature + 4)? != b"PE\0\0" {
        return None;
    }
    let number_of_sections = u16_at(data, signature + 6)? as usize;
    let optional_header_size = u16_at(data, signature + 20)? as usize;
    // The COFF file header is 20 bytes and directly follows the 4-byte signature.
    let sections_start = signature
        .checked_add(24)?
        .checked_add(optional_header_size)?;

    let mut regions = Vec::with_capacity(number_of_sections);
    for index in 0..number_of_sections {
        let header = sections_start.checked_add(index.checked_mul(40)?)?;
        let virtual_size = u32_at(data, header + 8)? as usize;
        let raw_size = u32_at(data, header + 16)? as usize;
        let raw_address = u32_at(data, header + 20)? as usize;
        let characteristics = u32_at(data, header + 36)? as usize;
        if raw_address == 0 || raw_size == 0 {
            continue;
        }
        // Section data is `min(SizeOfRawData, VirtualSize)` bytes of the range that exists in the
        // file; trailing alignment padding belongs to no section and must not be rewritten.
        let meaningful = virtual_size.min(raw_size).max(1);
        let start = raw_address;
        let end = raw_address
            .checked_add(meaningful)?
            .min(raw_address.checked_add(raw_size)?)
            .min(data.len());
        if end <= start {
            continue;
        }
        regions.push(MappedRegion {
            range: start..end,
            // IMAGE_SCN_MEM_EXECUTE
            executable: characteristics & 0x2000_0000 != 0,
        });
    }

    if regions.is_empty() {
        None
    } else {
        Some(regions)
    }
}

fn region_containing(
    regions: &[MappedRegion],
    offset: usize,
    executable_only: bool,
) -> Option<&MappedRegion> {
    regions
        .iter()
        .find(|region| region.range.contains(&offset) && (!executable_only || region.executable))
}

fn find_in_ranges(data: &[u8], needle: &[u8], mut accept: impl FnMut(usize) -> bool) -> Vec<usize> {
    let mut hits = Vec::new();
    if needle.is_empty() || data.len() < needle.len() {
        return hits;
    }
    for offset in 0..=(data.len() - needle.len()) {
        if data[offset..offset + needle.len()] == *needle && accept(offset) {
            hits.push(offset);
        }
    }
    hits
}

fn contains_window(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// True when `offset` starts the init thunk's store-and-fail sequence.
fn is_unixlib_init_site(data: &[u8], offset: usize) -> bool {
    if data.get(offset + 3..offset + 6) != Some(&STORE_TO_HANDLE_GLOBAL) {
        return false;
    }
    // `movq %rip+disp32, %rax` is 7 bytes, so the failure path starts no later than this window.
    let tail_start = offset + 10;
    data.get(tail_start..tail_start + FAILURE_PATH_WINDOW)
        .is_some_and(|tail| contains_window(tail, &STATUS_DLL_NOT_FOUND))
}

/// Retargets `bytes` onto the Unix-call ABI of a Wine runtime.
///
/// `Ok(None)` means nothing needed doing: the image is not a PE, does not use the Unix-call thunk at
/// all (an ordinary DLL), or already names exactly what the host exports. Rewriting only ever moves
/// from the dispatcher spelling to Apple's plain one, because that direction fits in the space the
/// longer name occupies; anything else is reported rather than approximated.
pub fn bridge_image(bytes: &mut [u8], host: WineUnixCallAbi) -> Result<Option<ImageBridge>> {
    // Whichever name the image actually binds to wins the comparison; a stray occurrence elsewhere is
    // not an import. The dispatcher spelling is the more specific marker, so it takes precedence when
    // both names appear in one binary.
    let bound_to_dispatcher = !find_symbols(bytes, DISPATCHER_SYMBOL).is_empty();
    let bound_to_plain = !find_symbols(bytes, PLAIN_SYMBOL).is_empty();
    if !bound_to_dispatcher && !bound_to_plain {
        // Not a Unix-call image at all: an ordinary DLL, or one already carrying the host's name only.
        return Ok(None);
    }
    let foreign = match host {
        WineUnixCallAbi::Plain => DISPATCHER_SYMBOL,
        WineUnixCallAbi::Dispatcher => PLAIN_SYMBOL,
    };

    // Already naming what the host exports: leave the binary byte-identical to Apple's.
    if (bound_to_dispatcher && host == WineUnixCallAbi::Dispatcher)
        || (!bound_to_dispatcher && host == WineUnixCallAbi::Plain)
    {
        return Ok(None);
    }
    let foreign_hits = find_symbols(bytes, foreign);
    if foreign_hits.is_empty() {
        return Ok(None);
    }

    let regions = mapped_regions(bytes).with_context(|| {
        format!(
            "image references `{}` but its PE section table could not be parsed, so it cannot be \
             retargeted onto a Wine exposing `{host}`",
            String::from_utf8_lossy(foreign)
        )
    })?;

    let symbol_sites: Vec<usize> = foreign_hits
        .iter()
        .filter(|offset| region_containing(&regions, **offset, false).is_some())
        .copied()
        .collect();
    if symbol_sites.is_empty() {
        bail!(
            "image references `{}` only outside of mapped sections; refusing to guess how to \
             retarget it onto a Wine exposing `{host}`",
            String::from_utf8_lossy(foreign)
        );
    }

    // The rename below shrinks the name, so it can only express dispatcher -> plain. Growing a name
    // would require relocating everything that follows it in the string table.
    if host != WineUnixCallAbi::Plain {
        bail!(
            "image binds to `{}` but this Wine exposes `{host}`; Nucleon can retarget binaries onto \
             Apple's GPTK base Wine, not away from it",
            String::from_utf8_lossy(foreign)
        );
    }

    // Collected before mutating: renaming must not create or destroy a later opcode match.
    let init_sites: Vec<usize> = find_in_ranges(bytes, &DEREFERENCE_RSI, |offset| {
        region_containing(&regions, offset, true).is_some() && is_unixlib_init_site(bytes, offset)
    });
    if init_sites.is_empty() {
        bail!(
            "image binds to `{}` but its Unix-call initialisation thunk does not match the shape \
             Nucleon knows, so it cannot be retargeted onto a Wine exposing `{host}`. Launching it \
             anyway fails with STATUS_DLL_INIT_FAILED (0xc0000142).",
            String::from_utf8_lossy(foreign)
        );
    }

    // The replacement is NUL-padded to the original length so every following name keeps its offset;
    // shrinking or growing the string would shift the whole table.
    for offset in &symbol_sites {
        let end = offset + DISPATCHER_SYMBOL.len();
        bytes[*offset..*offset + PLAIN_SYMBOL.len()].copy_from_slice(PLAIN_SYMBOL);
        for byte in &mut bytes[*offset + PLAIN_SYMBOL.len()..end] {
            *byte = 0;
        }
    }
    for offset in &init_sites {
        bytes[*offset..*offset + PASS_RSI_BY_VALUE.len()].copy_from_slice(&PASS_RSI_BY_VALUE);
    }

    let report = ImageBridge {
        symbol_rewrites: symbol_sites.len(),
        init_sites_patched: init_sites.len(),
        unreferenced_symbol_copies: foreign_hits.len() - symbol_sites.len(),
    };
    verify_bridge(bytes, &regions, host, &report)?;
    Ok(Some(report))
}

/// Re-reads the rewritten image to prove no stale reference or unrewritten thunk survives.
fn verify_bridge(
    bytes: &[u8],
    regions: &[MappedRegion],
    host: WineUnixCallAbi,
    report: &ImageBridge,
) -> Result<()> {
    let foreign = match host {
        WineUnixCallAbi::Plain => DISPATCHER_SYMBOL,
        WineUnixCallAbi::Dispatcher => PLAIN_SYMBOL,
    };
    let stale = find_symbols(bytes, foreign)
        .into_iter()
        .filter(|offset| region_containing(regions, *offset, false).is_some())
        .count();
    if stale > 0 {
        bail!(
            "ABI verification failed: references to `{}` remain in mapped sections after rewriting",
            String::from_utf8_lossy(foreign)
        );
    }
    if !find_in_ranges(bytes, &DEREFERENCE_RSI, |offset| {
        region_containing(regions, offset, true).is_some() && is_unixlib_init_site(bytes, offset)
    })
    .is_empty()
    {
        bail!("ABI verification failed: Unix-call init site(s) remain unrewritten");
    }
    if report.symbol_rewrites == 0 || report.init_sites_patched == 0 {
        bail!("ABI rewrite reported no work despite being asked to retarget an image");
    }
    Ok(())
}

/// Copies `src_dir` onto `dst_dir`, retargeting every image that does not already match `host`.
///
/// Mirrors [`crate::runner`]'s copy semantics (directories, files and symlinks; existing files are
/// replaced) and never writes to `src_dir`. Unlike a plain recursive copy this is not best-effort: an
/// image that cannot be reconciled with the host aborts the whole operation.
pub fn stage_windows_tree(
    src_dir: &Path,
    dst_dir: &Path,
    host: Option<WineUnixCallAbi>,
) -> Result<StageReport> {
    let mut report = StageReport::default();
    walk_windows_tree(src_dir, src_dir, Some(dst_dir), host, &mut report)?;
    Ok(report)
}

/// Reports what [`stage_windows_tree`] would do to `src_dir`, without touching the filesystem.
///
/// Exists so `nucleon gptk verify` can show what staging would change using exactly the same rules,
/// rather than a second implementation that could drift from them. If the host ABI is unavailable,
/// the result only establishes that files would be copied unchanged; it cannot establish pairing
/// compatibility.
pub fn inspect_windows_tree(src_dir: &Path, host: Option<WineUnixCallAbi>) -> Result<StageReport> {
    let mut report = StageReport::default();
    walk_windows_tree(src_dir, src_dir, None, host, &mut report)?;
    Ok(report)
}

/// Walks a GPTK Windows binary tree, rewriting images for `host` when `dst` is given.
///
/// `dst == None` makes the walk read-only: every rule runs and every report is filled, but nothing on
/// disk is created or modified. Staging and verification therefore cannot disagree about what a given
/// GPTK folder needs.
fn walk_windows_tree(
    root: &Path,
    dir: &Path,
    dst: Option<&Path>,
    host: Option<WineUnixCallAbi>,
    report: &mut StageReport,
) -> Result<()> {
    if let Some(dst) = dst {
        fs::create_dir_all(dst).with_context(|| format!("Failed to create {}", dst.display()))?;
    }
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry =
            entry.with_context(|| format!("Failed to read an entry in {}", dir.display()))?;
        let source = entry.path();
        let target = dst.map(|dst| dst.join(entry.file_name()));
        let file_type = entry
            .file_type()
            .with_context(|| format!("Failed to stat {}", source.display()))?;
        if file_type.is_dir() {
            walk_windows_tree(root, &source, target.as_deref(), host, report)?;
            continue;
        }
        if file_type.is_symlink() {
            #[cfg(unix)]
            if let Some(target) = target {
                let link_target = fs::read_link(&source)
                    .with_context(|| format!("Failed to read symlink {}", source.display()))?;
                remove_existing(&target)?;
                std::os::unix::fs::symlink(link_target, &target)
                    .with_context(|| format!("Failed to stage symlink {}", target.display()))?;
            }
            report.passthrough += 1;
            continue;
        }

        let mut contents =
            fs::read(&source).with_context(|| format!("Failed to read {}", source.display()))?;
        // Without a detected host ABI nothing is rewritten. That preserves Apple's binaries, but does
        // not establish that this GPTK distribution and Wine runtime form a compatible pair.
        let rewrite = match host {
            Some(host) => bridge_image(&mut contents, host).with_context(|| {
                format!(
                    "Cannot stage Apple GPTK binary {} for this Wine runtime",
                    relative(root, &source).display()
                )
            })?,
            None => None,
        };

        match rewrite {
            Some(bridge) => {
                if let Some(target) = target {
                    fs::write(&target, &contents)
                        .with_context(|| format!("Failed to write {}", target.display()))?;
                }
                report
                    .bridged
                    .push((relative(root, &source).display().to_string(), bridge));
            }
            None => {
                if let (Some(target), Some(_)) = (target, dst) {
                    remove_existing(&target)?;
                    fs::copy(&source, &target).with_context(|| {
                        format!(
                            "Failed to stage {} onto {}",
                            source.display(),
                            target.display()
                        )
                    })?;
                }
                report.passthrough += 1;
            }
        }
    }
    Ok(())
}

fn remove_existing(path: &Path) -> Result<()> {
    if (path.is_symlink() || path.is_file()) && fs::remove_file(path).is_err() {
        bail!("Failed to replace {}", path.display());
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT_FILE_OFFSET: usize = 0x400;
    const RDATA_FILE_OFFSET: usize = 0x800;

    fn write_symbol(data: &mut [u8], offset: usize, name: &[u8]) {
        let end = offset + name.len();
        data[offset..end].copy_from_slice(name);
        data[end] = 0;
    }

    /// Builds a minimal PE32+ image with `init_sites` init thunks and `vtable_sites` lookalike vtable
    /// loads in `.text`, plus `symbols` copies of the dispatcher name in `.rdata`.
    fn unixlib_image(init_sites: usize, vtable_sites: usize, symbols: usize) -> Vec<u8> {
        let mut data = vec![0u8; 0x1000];
        data[0x3c..0x40].copy_from_slice(&0x78u32.to_le_bytes());
        data[0x78..0x7c].copy_from_slice(b"PE\0\0");
        data[0x7e..0x80].copy_from_slice(&2u16.to_le_bytes()); // two sections
        data[0x8c..0x8e].copy_from_slice(&240u16.to_le_bytes()); // optional header size
        for (index, (name, addr, raw, size, chars)) in [
            (
                b".text\0\0\0",
                0x1000u32,
                TEXT_FILE_OFFSET as u32,
                0x400u32,
                0x6000_0020u32,
            ),
            (
                b".rdata\0\0",
                0x2000u32,
                RDATA_FILE_OFFSET as u32,
                0x400u32,
                0x4000_0040u32,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let header = 0x78 + 24 + 240 + index * 40;
            data[header..header + 8].copy_from_slice(name);
            data[header + 8..header + 12].copy_from_slice(&size.to_le_bytes());
            data[header + 12..header + 16].copy_from_slice(&addr.to_le_bytes());
            data[header + 16..header + 20].copy_from_slice(&size.to_le_bytes());
            data[header + 20..header + 24].copy_from_slice(&raw.to_le_bytes());
            data[header + 36..header + 40].copy_from_slice(&chars.to_le_bytes());
        }

        let mut text = TEXT_FILE_OFFSET;
        for _ in 0..init_sites {
            // The byte shape lifted from GPTK 4's dxgi.dll init thunk: set up the query, call through
            // the import table, test the status, then store-and-fail.
            data[text..text + 6].copy_from_slice(b"\x41\xb8\xe8\x03\x00\x00");
            data[text + 7..text + 9].copy_from_slice(&[0xff, 0x15]);
            data[text + 13] = 0x85;
            data[text + 15] = 0x13;
            let site = text + 16;
            data[site..site + 3].copy_from_slice(&DEREFERENCE_RSI);
            data[site + 3..site + 6].copy_from_slice(&STORE_TO_HANDLE_GLOBAL);
            let fail = site + 10;
            data[fail..fail + STATUS_DLL_NOT_FOUND.len()].copy_from_slice(&STATUS_DLL_NOT_FOUND);
            text = fail + 8;
        }
        for _ in 0..vtable_sites {
            // `mov rax,[rsi]; mov rcx,rsi; call qword [rax+0x10]` — a virtual method call. Same three
            // bytes as the init site, and it must survive untouched.
            data[text..text + 3].copy_from_slice(&DEREFERENCE_RSI);
            data[text + 3..text + 9].copy_from_slice(b"\x48\x89\xf1\xff\x50\x10");
            text += 9;
        }

        let mut rdata = RDATA_FILE_OFFSET;
        for _ in 0..symbols {
            write_symbol(&mut data, rdata, DISPATCHER_SYMBOL);
            rdata += DISPATCHER_SYMBOL.len() + 8;
        }

        // One copy outside every mapped section, mimicking Apple's COFF symbol table.
        let tail = data.len() - DISPATCHER_SYMBOL.len() - 1;
        write_symbol(&mut data, tail, DISPATCHER_SYMBOL);

        data
    }

    #[test]
    fn test_retargets_onto_apple_base_wine_without_touching_vtable_loads() {
        let mut image = unixlib_image(1, 4, 1);
        let original = image.clone();

        let bridge = bridge_image(&mut image, WineUnixCallAbi::Plain)
            .unwrap()
            .expect("image naming the dispatcher must be retargeted");
        assert_eq!(bridge.symbol_rewrites, 1);
        assert_eq!(bridge.init_sites_patched, 1);
        assert_eq!(bridge.unreferenced_symbol_copies, 1);

        let opcode_sites = find_in_ranges(&original, &DEREFERENCE_RSI, |_| true);
        assert_eq!(opcode_sites.len(), 5);
        // Exactly one opcode rewrite: the init thunk. The four vtable loads are byte-identical.
        let rewritten = opcode_sites
            .iter()
            .filter(|site| image[**site..**site + 3] == PASS_RSI_BY_VALUE)
            .count();
        assert_eq!(rewritten, 1);

        // The dispatcher name is gone from mapped sections; total size is unchanged.
        let regions = mapped_regions(&image).unwrap();
        assert!(
            find_in_ranges(&image, DISPATCHER_SYMBOL, |offset| region_containing(
                &regions, offset, false
            )
            .is_some())
            .is_empty()
        );
        assert_eq!(image.len(), original.len());

        // Nothing outside the thunk and the name moved.
        let changed: Vec<usize> = (0..image.len())
            .filter(|offset| {
                image[*offset] != original[*offset]
                    && !opcode_sites
                        .iter()
                        .copied()
                        .any(|site| (site..site + 3).contains(offset))
                    && !find_in_ranges(&original, DISPATCHER_SYMBOL, |_| true)
                        .into_iter()
                        .any(|site| (site..site + DISPATCHER_SYMBOL.len()).contains(offset))
            })
            .collect();
        assert!(changed.is_empty(), "unexpected extra edits at {changed:?}");
    }

    #[test]
    fn test_binaries_already_matching_the_host_are_left_byte_identical() {
        // A current Wine exports the dispatcher name that GPTK 4 already binds to: no rewrite, and in
        // particular no rename that would leave the thunk unable to resolve anything.
        let mut image = unixlib_image(1, 35, 1);
        let original = image.clone();
        assert!(
            bridge_image(&mut image, WineUnixCallAbi::Dispatcher)
                .unwrap()
                .is_none()
        );
        assert_eq!(image, original, "matching host must not be rewritten");
    }

    #[test]
    fn test_images_that_use_no_unix_calls_are_left_alone() {
        let mut image = unixlib_image(0, 3, 0);
        // Clear the trailing symbol-table copy so neither spelling remains.
        let tail_start = image.len() - 64;
        for byte in &mut image[tail_start..] {
            *byte = 0;
        }
        let original = image.clone();
        assert!(
            bridge_image(&mut image, WineUnixCallAbi::Plain)
                .unwrap()
                .is_none()
        );
        assert!(
            bridge_image(&mut image, WineUnixCallAbi::Dispatcher)
                .unwrap()
                .is_none()
        );
        assert_eq!(image, original);
    }

    #[test]
    fn test_non_pe_bytes_are_ignored() {
        let mut data = b"MJ\x00\x00not a pe image".to_vec();
        assert!(
            bridge_image(&mut data, WineUnixCallAbi::Plain)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_refuses_when_the_thunk_shape_is_unrecognised() {
        // Dispatcher name present, but the only matching opcode is a vtable load: guessing here would
        // corrupt NVAPI/NVNGX dispatch, so this has to be an error rather than a silent copy.
        let mut image = unixlib_image(0, 2, 1);
        let err = bridge_image(&mut image, WineUnixCallAbi::Plain)
            .unwrap_err()
            .to_string();
        assert!(err.contains("initialisation thunk"), "{err}");
    }

    #[test]
    fn test_never_rewrites_towards_a_name_the_host_does_not_offer() {
        // An image bound to Apple's plain name cannot be grown into the longer dispatcher name; that
        // pairing must fail rather than produce a binary whose thunk resolves nothing.
        let mut image = unixlib_image(1, 0, 0);
        for _ in 0..1 {
            // Replace both symbol copies with the plain spelling.
            let tail = image.len() - DISPATCHER_SYMBOL.len() - 1;
            write_symbol(&mut image, tail, PLAIN_SYMBOL);
        }
        write_symbol(&mut image, RDATA_FILE_OFFSET, PLAIN_SYMBOL);
        for byte in &mut image[RDATA_FILE_OFFSET + PLAIN_SYMBOL.len()
            ..RDATA_FILE_OFFSET + DISPATCHER_SYMBOL.len() + 1]
        {
            *byte = 0;
        }

        let err = bridge_image(&mut image, WineUnixCallAbi::Dispatcher)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not away from it"), "{err}");
    }

    #[test]
    fn test_host_abi_is_detected_from_the_runtimes_own_windows_images() {
        let dir = tempfile::tempdir().unwrap();
        let win_dir = dir.path().join("lib/wine/x86_64-windows");
        fs::create_dir_all(&win_dir).unwrap();

        // Upstream Wine: ntdll.dll binds to the dispatcher, beside longer names sharing its prefix.
        let mut upstream = vec![0u8; 512];
        write_symbol(&mut upstream, 32, DISPATCHER_SYMBOL);
        write_symbol(&mut upstream, 128, b"__wine_unix_call_funcs");
        fs::write(win_dir.join("ntdll.dll"), &upstream).unwrap();
        assert_eq!(
            detect_wine_unix_call_abi(dir.path()),
            Some(WineUnixCallAbi::Dispatcher)
        );

        // Apple's GPTK base Wine: no dispatcher name anywhere, only the plain entry point — and neither
        // appears in its ntdll.so, which is why the Unix libraries cannot answer this question.
        let mut apple_base = vec![0u8; 512];
        write_symbol(&mut apple_base, 32, PLAIN_SYMBOL);
        write_symbol(&mut apple_base, 128, b"__wine_unix_call_funcs");
        fs::write(win_dir.join("ntdll.dll"), &apple_base).unwrap();
        assert_eq!(
            detect_wine_unix_call_abi(dir.path()),
            Some(WineUnixCallAbi::Plain)
        );

        // An ntdll that predates the thunk falls through to an image that uses it.
        fs::write(win_dir.join("ntdll.dll"), b"unrelated").unwrap();
        let mut consumer = vec![0u8; 128];
        write_symbol(&mut consumer, 16, PLAIN_SYMBOL);
        fs::write(win_dir.join("win32u.dll"), &consumer).unwrap();
        assert_eq!(
            detect_wine_unix_call_abi(dir.path()),
            Some(WineUnixCallAbi::Plain)
        );

        // A runtime naming neither cannot be classified, and must not be rewritten.
        fs::write(win_dir.join("win32u.dll"), b"unrelated").unwrap();
        assert_eq!(detect_wine_unix_call_abi(dir.path()), None);
    }

    #[test]
    fn test_staging_tree_rewrites_only_when_the_host_needs_it() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x86_64-windows");
        let dst = dir.path().join("staged");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("dxgi.dll"), unixlib_image(1, 35, 1)).unwrap();
        fs::write(src.join("wine.ini"), b"not a binary").unwrap();

        let report = stage_windows_tree(&src, &dst, Some(WineUnixCallAbi::Plain)).unwrap();
        assert_eq!(report.bridged_count(), 1);
        assert_eq!(report.bridged[0].0, "dxgi.dll");
        assert_eq!(report.bridged[0].1.init_sites_patched, 1);
        assert_eq!(report.passthrough, 1); // wine.ini

        let staged = fs::read(dst.join("dxgi.dll")).unwrap();
        let original = fs::read(src.join("dxgi.dll")).unwrap();
        assert_ne!(staged, original, "source must not be rewritten in place");
        assert_eq!(
            fs::read_to_string(dst.join("wine.ini")).unwrap(),
            "not a binary"
        );

        // Same tree staged for a matching host: copied verbatim.
        let matched = stage_windows_tree(&src, &dst, Some(WineUnixCallAbi::Dispatcher)).unwrap();
        assert_eq!(matched.bridged_count(), 0);
        assert_eq!(fs::read(dst.join("dxgi.dll")).unwrap(), original);

        // Undetectable host: also verbatim.
        let unknown = stage_windows_tree(&src, &dst, None).unwrap();
        assert_eq!(unknown.bridged_count(), 0);
    }

    #[test]
    fn test_inspection_reports_exactly_what_staging_would_do_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x86_64-windows");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("dxgi.dll"), unixlib_image(1, 35, 1)).unwrap();
        let before: Vec<u8> = fs::read(src.join("dxgi.dll")).unwrap();

        let report = inspect_windows_tree(&src, Some(WineUnixCallAbi::Plain)).unwrap();
        assert_eq!(report.bridged_count(), 1, "inspection must see the rewrite");
        assert_eq!(
            fs::read(src.join("dxgi.dll")).unwrap(),
            before,
            "inspection must not touch files"
        );
        assert!(!dir.path().join("staged").exists());

        // Inspection and staging agree, which is the point of sharing one implementation.
        let staged = inspect_windows_tree(&src, Some(WineUnixCallAbi::Dispatcher)).unwrap();
        assert_eq!(staged.bridged_count(), 0);
        let unknown = inspect_windows_tree(&src, None).unwrap();
        assert_eq!(unknown.bridged_count(), 0);
        assert_eq!(unknown.passthrough, 1);
        assert_eq!(fs::read(src.join("dxgi.dll")).unwrap(), before);
    }

    #[test]
    fn test_staging_aborts_on_an_image_it_cannot_reconcile() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x86_64-windows");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("nvapi64.dll"), unixlib_image(0, 7, 1)).unwrap();

        let err = stage_windows_tree(
            &src,
            &dir.path().join("staged"),
            Some(WineUnixCallAbi::Plain),
        )
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("nvapi64.dll"), "{message}");
        assert!(message.contains("initialisation thunk"), "{message}");
    }

    /// Validates the rules against Apple's real GPTK binaries.
    ///
    /// Run with `NUCLEON_TEST_GPTK_DIR=<gptk redist dir> cargo test -- --ignored`.
    #[test]
    #[ignore = "requires a local Apple GPTK installation"]
    fn test_real_gptk_binaries_need_exactly_one_thunk_each() {
        let Ok(configured) = std::env::var("NUCLEON_TEST_GPTK_DIR") else {
            eprintln!("NUCLEON_TEST_GPTK_DIR not set; skipping");
            return;
        };
        let root = PathBuf::from(configured);
        let mut checked = 0usize;
        for entry in walkdir(&root) {
            let Ok(original) = fs::read(&entry) else {
                continue;
            };
            if !contains_window(&original, DISPATCHER_SYMBOL) {
                continue;
            }
            // Retargeting onto Apple's base Wine must find exactly one thunk per image…
            let mut bytes = original.clone();
            let bridge = bridge_image(&mut bytes, WineUnixCallAbi::Plain)
                .unwrap_or_else(|err| panic!("{}: {err:#}", entry.display()))
                .expect("legacy image must be retargeted");
            assert_eq!(
                bridge.init_sites_patched,
                1,
                "{}: expected exactly one init thunk",
                entry.display()
            );
            assert_eq!(bridge.symbol_rewrites, 1, "{}", entry.display());
            // …and staging onto a current Wine must leave the file untouched.
            let mut untouched = original.clone();
            assert!(
                bridge_image(&mut untouched, WineUnixCallAbi::Dispatcher)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(untouched, original, "{}: must not change", entry.display());
            checked += 1;
        }
        assert!(
            checked > 0,
            "no dispatcher images found under {}",
            root.display()
        );
    }

    fn walkdir(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(walkdir(&path));
                } else {
                    out.push(path);
                }
            }
        }
        out
    }
}
