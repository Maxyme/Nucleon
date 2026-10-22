#![forbid(unsafe_code)]

use crate::{runner, wine};
use aho_corasick::{AhoCorasick, MatchKind};
use object::Object;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Validate a standalone target before creating or updating a Wine prefix.
pub fn executable_guest(path: &Path) -> anyhow::Result<crate::runtime::GuestAbi> {
    use anyhow::{Context, ensure};
    use object::endian::LittleEndian as LE;
    let file =
        fs::File::open(path).with_context(|| format!("Read executable {}", path.display()))?;
    let cache = object::read::ReadCache::new(file);
    let image = object::File::parse(&cache)
        .with_context(|| format!("Invalid Windows executable {}", path.display()))?;
    ensure!(
        image.format() == object::BinaryFormat::Pe
            && image.kind() == object::ObjectKind::Executable,
        "Expected a Windows executable (PE), not a library or other file: {}",
        path.display()
    );
    // Hybrid AMD64 headers still contain ARM64 code. Do not classify them as an
    // ordinary x64 application merely because the COFF machine says AMD64.
    if let object::File::Pe64(pe) = &image
        && let Some(directory) = pe.data_directory(object::pe::IMAGE_DIRECTORY_ENTRY_LOAD_CONFIG)
        && directory.virtual_address.get(LE) != 0
    {
        let config = directory.data(&cache, &pe.section_table())?;
        let size = config
            .get(..4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
            .context("Truncated executable load configuration")?;
        ensure!(
            size <= config.len(),
            "Truncated executable load configuration"
        );
        ensure!(
            size < 208 || !config[200..208].iter().any(|b| *b != 0),
            "Unsupported ARM64EC/ARM64X executable: {}; choose an x86 or x64 executable",
            path.display()
        );
    }
    match image.architecture() {
        object::Architecture::I386 => Ok(crate::runtime::GuestAbi::X86),
        object::Architecture::X86_64 => Ok(crate::runtime::GuestAbi::X64),
        other => anyhow::bail!(
            "Unsupported executable architecture {other:?}: {}; choose an x86 or x64 executable",
            path.display()
        ),
    }
}

fn dll_matcher<'a>(dlls: impl IntoIterator<Item = &'a str>) -> AhoCorasick {
    AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .match_kind(MatchKind::Standard)
        .build(dlls)
        .expect("Static DLL patterns must compile")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetEngine {
    Auto,
    Gptk,
    KosmicKrisp,
    Dxmt,
    Dxvk,
    Vkd3d,
    WineStaging,
}

impl TargetEngine {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetEngine::Auto => "auto",
            TargetEngine::Gptk => "gptk",
            TargetEngine::KosmicKrisp => "kosmickrisp",
            TargetEngine::Dxmt => "dxmt",
            TargetEngine::Dxvk => "dxvk",
            TargetEngine::Vkd3d => "vkd3d",
            TargetEngine::WineStaging => "staging",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            TargetEngine::Auto => "Wine + Automatic Graphics Backend",
            TargetEngine::Gptk => "GPTK + Apple D3DMetal",
            TargetEngine::KosmicKrisp => "Wine + DXVK/VKD3D (DX10-DX12) + Mesa KosmicKrisp Vulkan",
            TargetEngine::Dxmt => "Wine + DXMT (DX11) + Apple Metal",
            TargetEngine::Dxvk => "Wine + DXVK (DX10-DX11) + Mesa KosmicKrisp Vulkan",
            TargetEngine::Vkd3d => "Wine + VKD3D-Proton (DX12) + Mesa KosmicKrisp Vulkan",
            TargetEngine::WineStaging => "Wine + WineD3D OpenGL",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().trim() {
            "auto" | "automatic" | "default" => Some(TargetEngine::Auto),
            "gptk" | "apple" | "d3dmetal" => Some(TargetEngine::Gptk),
            "dxmt" => Some(TargetEngine::Dxmt),
            "dxvk" => Some(TargetEngine::Dxvk),
            "vkd3d" | "vkd3d-proton" => Some(TargetEngine::Vkd3d),
            "kosmickrisp" | "kk" | "kosmic" | "mesa" | "vulkan" => Some(TargetEngine::KosmicKrisp),
            "staging" | "wine-staging" | "wine" => Some(TargetEngine::WineStaging),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphicsApi {
    DirectX12,
    DirectX11,
    DirectX10,
    DirectX9OrOlder,
    DirectX7OrOlder,
    Vulkan,
    OpenGL,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionEvidence {
    Imports,
    DelayImports,
    DllNameHeuristic,
    None,
}

impl DetectionEvidence {
    pub fn description(self) -> &'static str {
        match self {
            Self::Imports => "PE imports",
            Self::DelayImports => "PE delay-load imports",
            Self::DllNameHeuristic => "DLL-name heuristic",
            Self::None => "No graphics dependency found",
        }
    }
}

#[derive(Debug, Clone)]
pub struct GraphicsApiInfo {
    pub api: GraphicsApi,
    pub engine: TargetEngine,
    pub detected_dll: Option<String>,
    pub evidence: DetectionEvidence,
    pub binary: Option<PathBuf>,
}

/// Installed components used by automatic routing. Separate discovery from selection so the
/// fallback policy can be tested without consulting the user's machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoBackends {
    pub gptk: bool,
    pub dxmt: bool,
    pub dxvk: bool,
    pub vkd3d: bool,
    pub vulkan: bool,
    pub d7vk: bool,
}

impl AutoBackends {
    pub fn discover() -> Self {
        Self::discover_for_api(&GraphicsApi::DirectX11, true)
    }

    fn discover_for_api(api: &GraphicsApi, is64: bool) -> Self {
        let wine_root = wine::get_active_wine_runtime()
            .map(|runtime| runtime.root)
            .or_else(runner::find_gptk_runner);
        wine_root
            .map(|root| Self::discover_for(&root, api, is64, None, None))
            .unwrap_or_default()
    }

    /// Validate packages against the runtime, executable architecture, and per-game overrides.
    pub fn discover_for(
        root: &Path,
        api: &GraphicsApi,
        is64: bool,
        icd: Option<&Path>,
        dxvk_path: Option<&Path>,
    ) -> Self {
        crate::runtime::ResolvedRuntime::inspect(root)
            .and_then(|runtime| {
                let components =
                    crate::launch::GraphicsComponents::discover(&runtime, icd, dxvk_path)?;
                Ok(components.available(&runtime, api, is64))
            })
            .unwrap_or_default()
    }

    /// A preference policy, not a performance prediction. Every Vulkan translator also requires
    /// the Vulkan driver; GPTK and DXMT use Metal directly.
    pub fn select(&self, api: &GraphicsApi) -> anyhow::Result<TargetEngine> {
        Ok(match api {
            GraphicsApi::DirectX12 if self.gptk => TargetEngine::Gptk,
            GraphicsApi::DirectX12 if self.vkd3d && self.vulkan => TargetEngine::Vkd3d,
            GraphicsApi::DirectX12 => anyhow::bail!(
                "No usable DirectX 12 backend found. Configure GPTK with a D3DMetal-capable Wine, \
                 or install VKD3D-Proton and KosmicKrisp; WineD3D/OpenGL cannot serve this API."
            ),
            GraphicsApi::DirectX11 if self.dxmt => TargetEngine::Dxmt,
            GraphicsApi::DirectX11 if self.gptk => TargetEngine::Gptk,
            GraphicsApi::DirectX11 | GraphicsApi::DirectX10 | GraphicsApi::DirectX9OrOlder
                if self.dxvk && self.vulkan =>
            {
                TargetEngine::Dxvk
            }
            GraphicsApi::DirectX7OrOlder if self.d7vk && self.vulkan => TargetEngine::KosmicKrisp,
            GraphicsApi::Vulkan if self.vulkan => TargetEngine::KosmicKrisp,
            _ => TargetEngine::WineStaging,
        })
    }
}

impl GraphicsApiInfo {
    pub fn auto_engine(&self) -> anyhow::Result<TargetEngine> {
        let is64 = self
            .binary
            .as_deref()
            .map(is_pe_file_64_bit)
            .unwrap_or(true);
        AutoBackends::discover_for_api(&self.api, is64).select(&self.api)
    }
}

/// Inspects a PE binary (or directory) and detects the Graphics API and recommended engine.
pub fn detect_target_engine(exe_path: &Path) -> GraphicsApiInfo {
    // Inspect the requested directory itself, never neighboring game directories.
    if exe_path.is_dir() {
        return scan_directory_for_graphics_api(exe_path, 3)
            .unwrap_or_else(|| classify_libraries(&[]));
    }
    let primary = inspect_pe_file(exe_path).ok();
    if let Some(info) = &primary
        && info.api != GraphicsApi::Unknown
    {
        return info.clone();
    }

    // 2. If unknown (e.g. wrapper launcher), scan neighboring and subdirectories for game binaries
    if let Some(parent) = exe_path.parent()
        && let Some(info) = scan_directory_for_graphics_api(parent, 2)
    {
        return info;
    }

    primary.unwrap_or_else(|| classify_libraries(&[]))
}

pub fn inspect_pe_file(path: &Path) -> Result<GraphicsApiInfo, anyhow::Error> {
    // Own the bytes so concurrent file replacement/truncation cannot invalidate
    // a borrowed mapping while object parsing is in progress.
    let data = fs::read(path)?;
    let mut info = inspect_pe_bytes(&data)?;
    info.binary = Some(path.to_path_buf());
    Ok(info)
}

const GRAPHICS_DLLS: &[(&str, GraphicsApi, TargetEngine)] = &[
    ("d3d12.dll", GraphicsApi::DirectX12, TargetEngine::Gptk),
    ("d3d12core.dll", GraphicsApi::DirectX12, TargetEngine::Gptk),
    ("d3d11.dll", GraphicsApi::DirectX11, TargetEngine::Gptk),
    (
        "d3d10.dll",
        GraphicsApi::DirectX10,
        TargetEngine::WineStaging,
    ),
    (
        "d3d10_1.dll",
        GraphicsApi::DirectX10,
        TargetEngine::WineStaging,
    ),
    (
        "d3d10core.dll",
        GraphicsApi::DirectX10,
        TargetEngine::WineStaging,
    ),
    (
        "d3d9.dll",
        GraphicsApi::DirectX9OrOlder,
        TargetEngine::WineStaging,
    ),
    (
        "d3d8.dll",
        GraphicsApi::DirectX9OrOlder,
        TargetEngine::WineStaging,
    ),
    (
        "ddraw.dll",
        GraphicsApi::DirectX7OrOlder,
        TargetEngine::WineStaging,
    ),
    (
        "vulkan-1.dll",
        GraphicsApi::Vulkan,
        TargetEngine::KosmicKrisp,
    ),
    (
        "opengl32.dll",
        GraphicsApi::OpenGL,
        TargetEngine::WineStaging,
    ),
    // DXGI is shared by DX10, DX11, DX12, and renderer wrappers. It is not a version witness.
    ("dxgi.dll", GraphicsApi::Unknown, TargetEngine::WineStaging),
];

fn classify_libraries(libraries: &[(String, DetectionEvidence)]) -> GraphicsApiInfo {
    for (dll, api, engine) in GRAPHICS_DLLS {
        if let Some((_, evidence)) = libraries.iter().find(|(library, _)| library == dll) {
            return GraphicsApiInfo {
                api: api.clone(),
                engine: *engine,
                detected_dll: Some((*dll).into()),
                evidence: *evidence,
                binary: None,
            };
        }
    }
    GraphicsApiInfo {
        api: GraphicsApi::Unknown,
        engine: TargetEngine::WineStaging,
        detected_dll: None,
        evidence: DetectionEvidence::None,
        binary: None,
    }
}

fn pe_libraries<'data, Pe: object::read::pe::ImageNtHeaders>(
    file: &object::read::pe::PeFile<'data, Pe>,
) -> Vec<(String, DetectionEvidence)> {
    use object::{endian::LittleEndian, read::pe::ImageOptionalHeader};
    let mut libraries = Vec::new();
    let mut add = |name: &[u8], evidence| {
        let name = String::from_utf8_lossy(name);
        // Import names can contain a directory. Compare complete basenames, not substrings.
        let name = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        libraries.push((name, evidence));
    };
    // Read DLL descriptors directly: libraries imported only by ordinal still count.
    if let Ok(Some(table)) = file.import_table()
        && let Ok(mut descriptors) = table.descriptors()
    {
        while let Ok(Some(descriptor)) = descriptors.next() {
            if let Ok(name) = table.name(descriptor.name.get(LittleEndian)) {
                add(name, DetectionEvidence::Imports);
            }
        }
    }
    if let Ok(Some(table)) = file
        .data_directories()
        .delay_load_import_table(file.data(), &file.section_table())
        && let Ok(mut descriptors) = table.descriptors()
    {
        while let Ok(Some(descriptor)) = descriptors.next() {
            // Bit 0 selects RVAs; older descriptors store absolute VAs.
            // https://learn.microsoft.com/en-us/windows/win32/debug/pe-format#delay-load-import-tables-image-only
            let address = descriptor.dll_name_rva.get(LittleEndian);
            let address = match descriptor.attributes.get(LittleEndian) {
                1 => Some(address),
                0 => u64::from(address)
                    .checked_sub(file.nt_headers().optional_header().image_base())
                    .and_then(|rva| u32::try_from(rva).ok()),
                _ => None,
            };
            if let Some(address) = address
                && let Ok(name) = table.name(address)
            {
                add(name, DetectionEvidence::DelayImports);
            }
        }
    }
    libraries
}

/// Scan all graphics DLL names together, keeping complete filename boundaries and table order.
fn graphics_dll_hints(data: &[u8]) -> Vec<(String, DetectionEvidence)> {
    static MATCHER: OnceLock<AhoCorasick> = OnceLock::new();
    let matcher = MATCHER.get_or_init(|| dll_matcher(GRAPHICS_DLLS.iter().map(|(dll, _, _)| *dll)));
    let filename_char =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.');
    let mut found = [false; GRAPHICS_DLLS.len()];
    for hit in matcher.find_overlapping_iter(data) {
        if (hit.start() == 0 || !filename_char(data[hit.start() - 1]))
            && (hit.end() == data.len() || !filename_char(data[hit.end()]))
        {
            found[hit.pattern().as_usize()] = true;
        }
    }
    GRAPHICS_DLLS
        .iter()
        .zip(found)
        .filter(|(_, found)| *found)
        .map(|((dll, _, _), _)| ((*dll).to_string(), DetectionEvidence::DllNameHeuristic))
        .collect()
}

pub fn inspect_pe_bytes(data: &[u8]) -> Result<GraphicsApiInfo, anyhow::Error> {
    let libraries = match object::File::parse(data) {
        Ok(object::File::Pe32(file)) => pe_libraries(&file),
        Ok(object::File::Pe64(file)) => pe_libraries(&file),
        _ => Vec::new(),
    };
    let parsed = classify_libraries(&libraries);
    if parsed.api != GraphicsApi::Unknown {
        return Ok(parsed);
    }

    // Packed games and dynamically loaded renderers may have only non-graphics imports.
    // Report this evidence as a heuristic; it never overrides an explicit graphics import.
    let hints = graphics_dll_hints(data);
    let heuristic = classify_libraries(&hints);
    if heuristic.api != GraphicsApi::Unknown {
        return Ok(heuristic);
    }
    if parsed.detected_dll.is_some() {
        return Ok(parsed);
    }
    Ok(heuristic)
}

fn scan_directory_for_graphics_api(dir: &Path, depth: u32) -> Option<GraphicsApiInfo> {
    if depth == 0 {
        return None;
    }

    walkdir::WalkDir::new(dir)
        .follow_links(true)
        .max_depth(depth as usize)
        .sort_by_key(|entry| (entry.file_type().is_dir(), entry.file_name().to_os_string()))
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || ["binaries", "bin", "x64", "win64"]
                    .iter()
                    .any(|name| entry.file_name().eq_ignore_ascii_case(name))
        })
        .find_map(|entry| {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log::warn!("Graphics discovery skipped an entry: {error}");
                    return None;
                }
            };
            if !entry.file_type().is_file()
                || !entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.contains("unins") || name.contains("crash") || name.contains("report") {
                return None;
            }
            inspect_pe_file(entry.path())
                .ok()
                .filter(|info| info.api != GraphicsApi::Unknown)
        })
}

/// Inspects raw executable bytes to determine whether the binary is 64-bit (PE32+) or 32-bit (PE32).
pub fn is_pe_64_bit(data: &[u8]) -> bool {
    if let Ok(file) = object::File::parse(data) {
        return file.is_64();
    }
    // Fallback: check PE machine type in header directly
    if data.len() >= 0x40 {
        let pe_offset =
            u32::from_le_bytes([data[0x3c], data[0x3d], data[0x3e], data[0x3f]]) as usize;
        if pe_offset + 6 <= data.len() && &data[pe_offset..pe_offset + 4] == b"PE\0\0" {
            let machine = u16::from_le_bytes([data[pe_offset + 4], data[pe_offset + 5]]);
            return machine == 0x8664; // IMAGE_FILE_MACHINE_AMD64
        }
    }
    false
}

/// Inspects a target executable on disk to determine whether it is a 64-bit binary.
pub fn is_pe_file_64_bit(path: &Path) -> bool {
    // Safe cached reads avoid buffering the entire executable just for bitness.
    if let Ok(file) = fs::File::open(path) {
        let cache = object::read::ReadCache::new(file);
        if let Ok(image) = object::File::parse(&cache) {
            return image.is_64();
        }
    }
    if let Ok(data) = fs::read(path) {
        return is_pe_64_bit(&data);
    }
    // Modern games default to 64-bit if file cannot be read
    true
}

/// Prefer shader helpers shipped with a game when Wine's builtin implementations are incomplete.
/// Only known DirectX helper DLLs matching the executable's architecture are considered.
pub fn bundled_directx_helpers(directory: &Path, is64: bool) -> Vec<String> {
    let mut helpers = Vec::new();
    if let Ok(entries) = fs::read_dir(directory) {
        for path in entries.flatten().map(|entry| entry.path()) {
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_ascii_lowercase)
            else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".dll") else {
                continue;
            };
            let supported = [
                ("d3dx9_", 24..=43),
                ("d3dx10_", 33..=43),
                ("d3dx11_", 42..=43),
                ("d3dcompiler_", 33..=47),
            ]
            .iter()
            .any(|(prefix, versions)| {
                stem.strip_prefix(prefix)
                    .and_then(|v| v.parse::<u32>().ok())
                    .is_some_and(|v| versions.contains(&v))
            });
            if supported && crate::runtime_checks::pe(Some(&path), is64) {
                helpers.push(stem.to_string());
            }
        }
    }
    helpers.sort();
    helpers.dedup();
    helpers
}

/// Loader DLLs that link a title against the Steam client. A game that loads one of these calls
/// `SteamAPI_Init()` during startup, and titles that treat a failed handshake as fatal (Denuvo
/// builds in particular) exit or ask Steam to relaunch them before they ever reach graphics.
const STEAMWORKS_DLLS: [&str; 2] = ["steam_api.dll", "steam_api64.dll"];

/// Returns true when `data` (a PE image) links a Steamworks loader DLL.
///
/// The import table is authoritative, but anti-tamper wrappers such as Denuvo strip it and rebuild
/// it at runtime, so the name scan below is what actually catches those titles.
pub fn image_uses_steamworks(data: &[u8]) -> bool {
    if let Ok(file) = object::File::parse(data)
        && let Ok(imports) = file.imports()
    {
        for import in imports {
            let lib = String::from_utf8_lossy(import.library()).to_ascii_lowercase();
            if STEAMWORKS_DLLS.iter().any(|dll| lib.ends_with(dll)) {
                return true;
            }
        }
    }

    static MATCHER: OnceLock<AhoCorasick> = OnceLock::new();
    MATCHER
        .get_or_init(|| dll_matcher(STEAMWORKS_DLLS))
        .is_match(data)
}

/// Whether the executable at `path` expects a running Steam client to complete its startup.
///
/// Detection is deliberately generous: launching through the prefix's `steam.exe` shim costs one
/// extra process and is invisible to games that do not need it, while a missing handshake kills
/// protected titles outright.
pub fn file_uses_steamworks(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if STEAMWORKS_DLLS.iter().any(|dll| parent.join(dll).is_file()) {
        return true;
    }

    match fs::read(path) {
        Ok(data) => image_uses_steamworks(&data),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PE32/PE32+ fixture with actual import descriptors and ordinal thunks.
    fn pe_with_libraries(is64: bool, imports: &[&str], delays: &[&str]) -> Vec<u8> {
        let mut data = vec![0u8; 1536];
        let put16 = |data: &mut [u8], offset: usize, value: u16| {
            data[offset..offset + 2].copy_from_slice(&value.to_le_bytes())
        };
        let put32 = |data: &mut [u8], offset: usize, value: u32| {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes())
        };
        data[..2].copy_from_slice(b"MZ");
        put32(&mut data, 60, 128);
        data[128..132].copy_from_slice(b"PE\0\0");
        put16(&mut data, 132, if is64 { 0x8664 } else { 0x14c });
        put16(&mut data, 134, 1);
        let opt_size = if is64 { 240 } else { 224 };
        put16(&mut data, 148, opt_size as u16);
        let opt = 152;
        put16(&mut data, opt, if is64 { 0x20b } else { 0x10b });
        put32(&mut data, opt + 60, 512);
        let directories = opt + if is64 { 112 } else { 96 };
        put32(&mut data, directories - 4, 16);
        let section = opt + opt_size;
        data[section..section + 6].copy_from_slice(b".idata");
        for (offset, value) in [
            (8, 1024),
            (12, 4096),
            (16, 1024),
            (20, 512),
            (36, 0x40000040),
        ] {
            put32(&mut data, section + offset, value);
        }
        if !imports.is_empty() {
            put32(&mut data, directories + 8, 4096);
            put32(
                &mut data,
                directories + 12,
                ((imports.len() + 1) * 20) as u32,
            );
        }
        if !delays.is_empty() {
            put32(&mut data, directories + 13 * 8, 4096 + 256);
            put32(
                &mut data,
                directories + 13 * 8 + 4,
                ((delays.len() + 1) * 32) as u32,
            );
        }
        for (index, name) in imports.iter().chain(delays).enumerate() {
            let name_offset = 512 + 512 + index * 64;
            data[name_offset..name_offset + name.len()].copy_from_slice(name.as_bytes());
            let name_rva = (name_offset - 512 + 4096) as u32;
            let thunk = 512 + 896 + index * 16;
            let thunk_rva = (thunk - 512 + 4096) as u32;
            if is64 {
                data[thunk..thunk + 8].copy_from_slice(&0x8000000000000001u64.to_le_bytes());
            } else {
                put32(&mut data, thunk, 0x80000001);
            }
            if index < imports.len() {
                let descriptor = 512 + index * 20;
                put32(&mut data, descriptor, thunk_rva);
                put32(&mut data, descriptor + 12, name_rva);
                put32(&mut data, descriptor + 16, thunk_rva);
            } else {
                let descriptor = 512 + 256 + (index - imports.len()) * 32;
                put32(&mut data, descriptor, 1);
                put32(&mut data, descriptor + 4, name_rva);
                put32(&mut data, descriptor + 12, thunk_rva);
                put32(&mut data, descriptor + 16, thunk_rva);
            }
        }
        data
    }

    #[test]
    fn bundled_shader_helpers_require_a_supported_name_and_matching_pe_architecture() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("D3DX9_43.dll"),
            pe_with_libraries(false, &[], &[]),
        )
        .unwrap();
        fs::write(
            dir.path().join("D3DCompiler_43.dll"),
            pe_with_libraries(false, &[], &[]),
        )
        .unwrap();
        fs::write(
            dir.path().join("d3dx11_43.dll"),
            pe_with_libraries(true, &[], &[]),
        )
        .unwrap();
        fs::write(
            dir.path().join("d3dcompiler_99.dll"),
            pe_with_libraries(false, &[], &[]),
        )
        .unwrap();
        fs::write(dir.path().join("d3dx9_42.dll"), b"not a PE").unwrap();
        assert_eq!(
            bundled_directx_helpers(dir.path(), false),
            ["d3dcompiler_43", "d3dx9_43"]
        );
        assert_eq!(bundled_directx_helpers(dir.path(), true), ["d3dx11_43"]);
    }

    #[test]
    fn explicit_dx10_imports_take_priority_over_shared_dxgi() {
        for is64 in [false, true] {
            for dll in ["d3d10.dll", "d3d10_1.dll", "d3d10core.dll"] {
                for delay in [false, true] {
                    let data = if delay {
                        pe_with_libraries(is64, &["dxgi.dll"], &[dll])
                    } else {
                        pe_with_libraries(is64, &["dxgi.dll", dll], &[])
                    };
                    let info = inspect_pe_bytes(&data).unwrap();
                    assert_eq!(info.api, GraphicsApi::DirectX10);
                    assert_eq!(info.detected_dll.as_deref(), Some(dll));
                    assert_eq!(
                        info.evidence,
                        if delay {
                            DetectionEvidence::DelayImports
                        } else {
                            DetectionEvidence::Imports
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn dxgi_alone_is_ambiguous_and_never_claims_dx11() {
        for data in [
            pe_with_libraries(true, &["dxgi.dll"], &[]),
            b"MZ\0dxgi.dll\0".to_vec(),
        ] {
            let info = inspect_pe_bytes(&data).unwrap();
            assert_eq!(info.api, GraphicsApi::Unknown);
            assert_eq!(info.detected_dll.as_deref(), Some("dxgi.dll"));
        }
    }

    #[test]
    fn legacy_delay_descriptors_convert_virtual_addresses_to_rvas() {
        let mut data = pe_with_libraries(false, &["kernel32.dll"], &["d3d10_1.dll"]);
        let base = 0x400000u32;
        data[152 + 28..152 + 32].copy_from_slice(&base.to_le_bytes());
        let descriptor = 512 + 256;
        data[descriptor..descriptor + 4].copy_from_slice(&0u32.to_le_bytes());
        let name = u32::from_le_bytes(data[descriptor + 4..descriptor + 8].try_into().unwrap());
        data[descriptor + 4..descriptor + 8].copy_from_slice(&(base + name).to_le_bytes());
        let info = inspect_pe_bytes(&data).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX10);
        assert_eq!(info.evidence, DetectionEvidence::DelayImports);
    }

    #[test]
    fn regular_and_delay_imports_use_the_highest_explicit_directx_version() {
        for is64 in [false, true] {
            let data = pe_with_libraries(is64, &["d3d10.dll", "D3D11.DLL"], &["d3d12.dll"]);
            let info = inspect_pe_bytes(&data).unwrap();
            assert_eq!(info.api, GraphicsApi::DirectX12);
            assert_eq!(info.evidence, DetectionEvidence::DelayImports);
            let data = pe_with_libraries(is64, &["d3d10.dll"], &["D3D11.DLL"]);
            assert_eq!(inspect_pe_bytes(&data).unwrap().api, GraphicsApi::DirectX11);
        }
    }

    #[test]
    fn heuristics_do_not_override_parsed_graphics_or_match_similar_filenames() {
        let mut data = pe_with_libraries(true, &["d3d10_1.dll", "dxgi.dll"], &[]);
        data.extend_from_slice(b"\0d3d12.dll\0");
        let info = inspect_pe_bytes(&data).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX10);
        assert_eq!(info.evidence, DetectionEvidence::Imports);
        for name in [
            "notd3d11.dll",
            "d3d12.dll.backup",
            "fake_dxgi.dll",
            "d3dcompiler_47.dll",
        ] {
            assert_eq!(
                inspect_pe_bytes(&pe_with_libraries(true, &[name], &[]))
                    .unwrap()
                    .api,
                GraphicsApi::Unknown,
                "{name}"
            );
        }
        // Dynamic loading can coexist with an otherwise valid, non-graphics import table.
        let mut dynamic = pe_with_libraries(true, &["kernel32.dll"], &[]);
        dynamic.extend_from_slice(b"\0D3D11.DLL\0");
        let info = inspect_pe_bytes(&dynamic).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX11);
        assert_eq!(info.evidence, DetectionEvidence::DllNameHeuristic);
    }

    #[test]
    fn graphics_matcher_handles_case_paths_binary_boundaries_and_later_valid_matches() {
        for (dll, api, _) in GRAPHICS_DLLS {
            let upper = dll.to_ascii_uppercase();
            for bytes in [
                upper.as_bytes().to_vec(),
                format!("C:\\Windows\\System32\\{upper}\0").into_bytes(),
                [vec![0xff], upper.as_bytes().to_vec(), vec![0xfe]].concat(),
            ] {
                let info = inspect_pe_bytes(&bytes).unwrap();
                assert_eq!(&info.api, api);
                assert_eq!(info.detected_dll.as_deref(), Some(*dll));
                assert_eq!(info.evidence, DetectionEvidence::DllNameHeuristic);
            }
            for invalid in [
                format!("not{dll}"),
                format!("_{dll}"),
                format!("-{dll}"),
                format!("{dll}.backup"),
                format!("{dll}x"),
                format!("{dll}_"),
            ] {
                assert!(
                    graphics_dll_hints(invalid.as_bytes()).is_empty(),
                    "{invalid}"
                );
                let later = format!("{invalid}\0{upper}\0{dll}");
                assert_eq!(
                    graphics_dll_hints(later.as_bytes()),
                    vec![((*dll).to_string(), DetectionEvidence::DllNameHeuristic)]
                );
            }
        }
    }

    #[test]
    fn graphics_matcher_keeps_api_priority_and_deduplicates_repeated_names() {
        let data = b"opengl32.dll\0d3d9.dll\0dxgi.dll\0D3D12.DLL\0d3d12.dll";
        let names = graphics_dll_hints(data)
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["d3d12.dll", "d3d9.dll", "opengl32.dll", "dxgi.dll"]);
        assert_eq!(inspect_pe_bytes(data).unwrap().api, GraphicsApi::DirectX12);
        // Steam detection intentionally remains generous for packed binaries.
        assert!(image_uses_steamworks(b"_STEAM_API64.DLL.backup"));
        assert!(!image_uses_steamworks(b"steam_api65.dll"));
    }

    #[test]
    fn folder_detection_stays_within_requested_game_and_skips_crash_reporters() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("Game");
        fs::create_dir(&game).unwrap();
        fs::write(
            dir.path().join("neighbor.exe"),
            pe_with_libraries(true, &["d3d12.dll"], &[]),
        )
        .unwrap();
        assert_eq!(detect_target_engine(&game).api, GraphicsApi::Unknown);
        fs::write(
            game.join("a-crashreporter.exe"),
            pe_with_libraries(true, &["d3d12.dll"], &[]),
        )
        .unwrap();
        fs::write(
            game.join("game.exe"),
            pe_with_libraries(true, &["d3d10.dll", "dxgi.dll"], &[]),
        )
        .unwrap();
        assert_eq!(detect_target_engine(&game).api, GraphicsApi::DirectX10);
        let wrapper = game.join("launcher.exe");
        fs::write(&wrapper, pe_with_libraries(true, &["kernel32.dll"], &[])).unwrap();
        let detected = detect_target_engine(&wrapper);
        assert_eq!(detected.api, GraphicsApi::DirectX10);
        assert_eq!(detected.binary, Some(game.join("game.exe")));
    }

    #[test]
    fn folder_discovery_preserves_root_priority_allowed_folders_and_depth() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("Binaries/Win64/game.exe");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&nested, pe_with_libraries(true, &["d3d12.dll"], &[])).unwrap();
        let unrelated = dir.path().join("another-game/game.exe");
        fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        fs::write(unrelated, pe_with_libraries(true, &["d3d11.dll"], &[])).unwrap();
        assert!(scan_directory_for_graphics_api(dir.path(), 2).is_none());
        assert_eq!(
            scan_directory_for_graphics_api(dir.path(), 3)
                .unwrap()
                .binary,
            Some(nested.clone())
        );
        let root_game = dir.path().join("z-game.exe");
        fs::write(&root_game, pe_with_libraries(true, &["d3d10.dll"], &[])).unwrap();
        assert_eq!(
            scan_directory_for_graphics_api(dir.path(), 3)
                .unwrap()
                .binary,
            Some(root_game)
        );
        #[cfg(unix)]
        {
            fs::remove_file(dir.path().join("z-game.exe")).unwrap();
            fs::remove_file(&nested).unwrap();
            std::os::unix::fs::symlink("..", dir.path().join("Binaries/bin")).unwrap();
            assert!(scan_directory_for_graphics_api(dir.path(), 3).is_none());
        }
    }

    #[test]
    fn test_image_uses_steamworks_detects_protected_binaries() {
        // A Denuvo-style binary has no readable import table, so only the name scan can see this.
        let packed = b"MZ\x90\x00...packed\0SteamAPI_Init\x00steam_api64.dll\x00d3d11.dll";
        assert!(image_uses_steamworks(packed));

        // A title without Steamworks must not be wrapped just because it happens to be a game.
        let bare = b"MZ\x90\x00...SomeHeader\0d3d9.dll\x00opengl32.dll";
        assert!(!image_uses_steamworks(bare));

        // Case differences must not hide the dependency.
        assert!(image_uses_steamworks(b"MZ...STEAM_API64.DLL...d3d11.dll"));
    }

    #[test]
    fn test_detection_priorities() {
        // Simulated binary containing d3d12.dll
        let bytes_dx12 = b"MZ\x90\x00...SomeHeader\0d3d12.dll\x00kernel32.dll\x00";
        let info = inspect_pe_bytes(bytes_dx12).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX12);
        assert_eq!(info.engine, TargetEngine::Gptk);

        // Simulated binary containing d3d11.dll
        let bytes_dx11 = b"MZ\x90\x00...SomeHeader\0d3d11.dll\x00dxgi.dll\x00";
        let info = inspect_pe_bytes(bytes_dx11).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX11);
        assert_eq!(info.engine, TargetEngine::Gptk);

        // Simulated binary containing d3d10.dll
        let bytes_dx10 = b"MZ\x90\x00...SomeHeader\0d3d10.dll\x00user32.dll\x00";
        let info = inspect_pe_bytes(bytes_dx10).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX10);
        assert_eq!(info.engine, TargetEngine::WineStaging);

        // Simulated binary containing d3d9.dll
        let bytes_dx9 = b"MZ\x90\x00...SomeHeader\0d3d9.dll\x00user32.dll\x00";
        let info = inspect_pe_bytes(bytes_dx9).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX9OrOlder);
        assert_eq!(info.engine, TargetEngine::WineStaging);

        // Simulated binary containing vulkan-1.dll
        let bytes_vk = b"MZ\x90\x00...SomeHeader\0vulkan-1.dll\x00kernel32.dll\x00";
        let info = inspect_pe_bytes(bytes_vk).unwrap();
        assert_eq!(info.api, GraphicsApi::Vulkan);
        assert_eq!(info.engine, TargetEngine::KosmicKrisp);

        // Simulated binary containing opengl32.dll
        let bytes_gl = b"MZ\x90\x00...SomeHeader\0opengl32.dll\x00gdi32.dll\x00";
        let info = inspect_pe_bytes(bytes_gl).unwrap();
        assert_eq!(info.api, GraphicsApi::OpenGL);
        assert_eq!(info.engine, TargetEngine::WineStaging);

        // Simulated binary containing ddraw.dll
        let bytes_ddraw = b"MZ\x90\x00...SomeHeader\0ddraw.dll\x00kernel32.dll\x00";
        let info = inspect_pe_bytes(bytes_ddraw).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX7OrOlder);
        assert_eq!(info.detected_dll, Some("ddraw.dll".into()));
    }

    #[test]
    fn test_target_engine_parsing() {
        assert_eq!(TargetEngine::parse("auto"), Some(TargetEngine::Auto));
        assert_eq!(TargetEngine::parse("automatic"), Some(TargetEngine::Auto));
        assert_eq!(TargetEngine::parse("default"), Some(TargetEngine::Auto));
        assert_eq!(TargetEngine::parse("gptk"), Some(TargetEngine::Gptk));
        assert_eq!(TargetEngine::parse("apple"), Some(TargetEngine::Gptk));
        assert_eq!(TargetEngine::parse("dxvk"), Some(TargetEngine::Dxvk));
        assert_eq!(TargetEngine::parse("vkd3d"), Some(TargetEngine::Vkd3d));
        assert_eq!(
            TargetEngine::parse("kosmickrisp"),
            Some(TargetEngine::KosmicKrisp)
        );
        assert_eq!(TargetEngine::parse("kk"), Some(TargetEngine::KosmicKrisp));
        assert_eq!(
            TargetEngine::parse("kosmic"),
            Some(TargetEngine::KosmicKrisp)
        );
        assert_eq!(TargetEngine::parse("mesa"), Some(TargetEngine::KosmicKrisp));
        assert_eq!(
            TargetEngine::parse("vulkan"),
            Some(TargetEngine::KosmicKrisp)
        );
        assert_eq!(
            TargetEngine::parse("staging"),
            Some(TargetEngine::WineStaging)
        );
        assert_eq!(TargetEngine::parse("wine"), Some(TargetEngine::WineStaging));
        assert_eq!(TargetEngine::parse("unknown_engine"), None);
    }

    #[test]
    fn test_target_engine_display_names() {
        assert_eq!(
            TargetEngine::Auto.display_name(),
            "Wine + Automatic Graphics Backend"
        );
        assert_eq!(TargetEngine::Gptk.display_name(), "GPTK + Apple D3DMetal");
        assert_eq!(
            TargetEngine::KosmicKrisp.display_name(),
            "Wine + DXVK/VKD3D (DX10-DX12) + Mesa KosmicKrisp Vulkan"
        );
        assert_eq!(
            TargetEngine::Dxmt.display_name(),
            "Wine + DXMT (DX11) + Apple Metal"
        );
        assert_eq!(
            TargetEngine::Dxvk.display_name(),
            "Wine + DXVK (DX10-DX11) + Mesa KosmicKrisp Vulkan"
        );
        assert_eq!(
            TargetEngine::Vkd3d.display_name(),
            "Wine + VKD3D-Proton (DX12) + Mesa KosmicKrisp Vulkan"
        );
        assert_eq!(
            TargetEngine::WineStaging.display_name(),
            "Wine + WineD3D OpenGL"
        );
    }

    #[test]
    fn test_auto_backend_preferences_and_fallbacks() {
        let all = AutoBackends {
            gptk: true,
            dxmt: true,
            dxvk: true,
            vkd3d: true,
            vulkan: true,
            d7vk: true,
        };
        for (api, expected) in [
            (GraphicsApi::DirectX12, TargetEngine::Gptk),
            (GraphicsApi::DirectX11, TargetEngine::Dxmt),
            (GraphicsApi::DirectX10, TargetEngine::Dxvk),
            (GraphicsApi::DirectX9OrOlder, TargetEngine::Dxvk),
            (GraphicsApi::DirectX7OrOlder, TargetEngine::KosmicKrisp),
            (GraphicsApi::Vulkan, TargetEngine::KosmicKrisp),
            (GraphicsApi::OpenGL, TargetEngine::WineStaging),
            (GraphicsApi::Unknown, TargetEngine::WineStaging),
        ] {
            assert_eq!(all.select(&api).unwrap(), expected);
            let absent = AutoBackends::default().select(&api);
            if api == GraphicsApi::DirectX12 {
                assert!(absent.is_err());
            } else {
                assert_eq!(absent.unwrap(), TargetEngine::WineStaging);
            }
        }
        let no_gptk = AutoBackends { gptk: false, ..all };
        assert_eq!(
            no_gptk.select(&GraphicsApi::DirectX12).unwrap(),
            TargetEngine::Vkd3d
        );
        for missing in [
            AutoBackends {
                vkd3d: false,
                ..no_gptk
            },
            AutoBackends {
                vulkan: false,
                ..no_gptk
            },
        ] {
            assert!(missing.select(&GraphicsApi::DirectX12).is_err());
        }
        let no_dxmt = AutoBackends { dxmt: false, ..all };
        assert_eq!(
            no_dxmt.select(&GraphicsApi::DirectX11).unwrap(),
            TargetEngine::Gptk
        );
        let vulkan_only = AutoBackends {
            gptk: false,
            ..no_dxmt
        };
        assert_eq!(
            vulkan_only.select(&GraphicsApi::DirectX11).unwrap(),
            TargetEngine::Dxvk
        );
        for available in [
            AutoBackends {
                dxvk: false,
                ..vulkan_only
            },
            AutoBackends {
                vulkan: false,
                ..vulkan_only
            },
        ] {
            assert_eq!(
                available.select(&GraphicsApi::DirectX11).unwrap(),
                TargetEngine::WineStaging
            );
            assert_eq!(
                available.select(&GraphicsApi::DirectX9OrOlder).unwrap(),
                TargetEngine::WineStaging
            );
        }
        assert_eq!(
            AutoBackends { d7vk: false, ..all }
                .select(&GraphicsApi::DirectX7OrOlder)
                .unwrap(),
            TargetEngine::WineStaging
        );
    }

    #[test]
    fn test_is_pe_64_bit() {
        // Construct minimal mock PE headers
        let mut pe64 = vec![0u8; 0x100];
        pe64[0] = b'M';
        pe64[1] = b'Z';
        pe64[0x3c] = 0x80; // PE offset at 0x80
        pe64[0x80] = b'P';
        pe64[0x81] = b'E';
        pe64[0x82] = 0;
        pe64[0x83] = 0;
        pe64[0x84] = 0x64; // IMAGE_FILE_MACHINE_AMD64 (0x8664)
        pe64[0x85] = 0x86;
        assert!(is_pe_64_bit(&pe64));

        let mut pe32 = vec![0u8; 0x100];
        pe32[0] = b'M';
        pe32[1] = b'Z';
        pe32[0x3c] = 0x80;
        pe32[0x80] = b'P';
        pe32[0x81] = b'E';
        pe32[0x82] = 0;
        pe32[0x83] = 0;
        pe32[0x84] = 0x4c; // IMAGE_FILE_MACHINE_I386 (0x014c)
        pe32[0x85] = 0x01;
        assert!(!is_pe_64_bit(&pe32));
    }
}
