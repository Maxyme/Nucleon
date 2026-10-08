use object::Object;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetEngine {
    Auto,
    Gptk,
    KosmicKrisp,
    WineStaging,
}

impl TargetEngine {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetEngine::Auto => "auto",
            TargetEngine::Gptk => "gptk",
            TargetEngine::KosmicKrisp => "kosmickrisp",
            TargetEngine::WineStaging => "staging",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            TargetEngine::Auto => "Wine + Automatic Graphics Backend",
            TargetEngine::Gptk => "GPTK + Apple D3DMetal",
            TargetEngine::KosmicKrisp => "Wine + Mesa KosmicKrisp Vulkan",
            TargetEngine::WineStaging => "Wine + WineD3D OpenGL",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().trim() {
            "auto" | "automatic" | "default" => Some(TargetEngine::Auto),
            "gptk" | "apple" | "d3dmetal" => Some(TargetEngine::Gptk),
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

#[derive(Debug, Clone)]
pub struct GraphicsApiInfo {
    pub api: GraphicsApi,
    pub engine: TargetEngine,
    pub detected_dll: Option<String>,
}

impl GraphicsApiInfo {
    /// Resolves the optimal graphics translation engine for the Wine runtime (Wine only).
    /// DirectX 11, DirectX 12, Vulkan, and DirectDraw/DX1–7 (via D7VK) route to KosmicKrisp (Vulkan 1.4);
    /// DirectX 9, DirectX 10, and OpenGL route to WineStaging (WineD3D / OpenGL).
    pub fn wine_engine(&self) -> TargetEngine {
        match self.api {
            GraphicsApi::DirectX12
            | GraphicsApi::DirectX11
            | GraphicsApi::Vulkan
            | GraphicsApi::DirectX7OrOlder => {
                if crate::runner::is_kosmickrisp_installed() {
                    TargetEngine::KosmicKrisp
                } else {
                    TargetEngine::WineStaging
                }
            }
            GraphicsApi::DirectX10
            | GraphicsApi::DirectX9OrOlder
            | GraphicsApi::OpenGL
            | GraphicsApi::Unknown => TargetEngine::WineStaging,
        }
    }
}

/// Inspects a PE binary (or directory) and detects the Graphics API and recommended engine.
pub fn detect_target_engine(exe_path: &Path) -> GraphicsApiInfo {
    // 1. Try to inspect the primary executable
    if let Ok(info) = inspect_pe_file(exe_path) {
        if info.api != GraphicsApi::Unknown {
            return info;
        }
    }

    // 2. If unknown (e.g. wrapper launcher), scan neighboring and subdirectories for game binaries
    if let Some(parent) = exe_path.parent() {
        if let Some(info) = scan_directory_for_graphics_api(parent, 2) {
            return info;
        }
    }

    // Default to GPTK for modern compatibility if ambiguous
    GraphicsApiInfo {
        api: GraphicsApi::Unknown,
        engine: TargetEngine::Gptk,
        detected_dll: None,
    }
}

pub fn inspect_pe_file(path: &Path) -> Result<GraphicsApiInfo, anyhow::Error> {
    let data = fs::read(path)?;
    inspect_pe_bytes(&data)
}

pub fn inspect_pe_bytes(data: &[u8]) -> Result<GraphicsApiInfo, anyhow::Error> {
    let mut imported_dlls = Vec::new();

    if let Ok(file) = object::File::parse(data) {
        if let Ok(imports) = file.imports() {
            for import in imports {
                let lib = String::from_utf8_lossy(import.library()).to_lowercase();
                imported_dlls.push(lib);
            }
        }
    }

    // Fallback: If import table parsing was empty or truncated, scan ASCII / UTF-8 byte sequences
    if imported_dlls.is_empty() {
        let text = String::from_utf8_lossy(data).to_lowercase();
        for dll in &[
            "d3d12.dll",
            "d3d11.dll",
            "dxgi.dll",
            "d3d10.dll",
            "d3d10_1.dll",
            "d3d10core.dll",
            "d3d9.dll",
            "d3d8.dll",
            "ddraw.dll",
            "vulkan-1.dll",
            "opengl32.dll",
        ] {
            if text.contains(dll) {
                imported_dlls.push(dll.to_string());
            }
        }
    }

    // Evaluate priorities:
    // 1. DirectX 12 (highest priority modern API)
    if imported_dlls.iter().any(|d| d.contains("d3d12")) {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::DirectX12,
            engine: TargetEngine::Gptk,
            detected_dll: Some("d3d12.dll".into()),
        });
    }

    // 2. DirectX 11
    if imported_dlls.iter().any(|d| d.contains("d3d11")) {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::DirectX11,
            engine: TargetEngine::Gptk,
            detected_dll: Some("d3d11.dll".into()),
        });
    }

    // 3. DXGI without explicit d3d12/d3d11 is almost always DX11/12
    if imported_dlls.iter().any(|d| d.contains("dxgi")) {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::DirectX11,
            engine: TargetEngine::Gptk,
            detected_dll: Some("dxgi.dll".into()),
        });
    }

    // 4. DirectX 10
    if imported_dlls.iter().any(|d| d.contains("d3d10")) {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::DirectX10,
            engine: TargetEngine::WineStaging,
            detected_dll: Some("d3d10.dll".into()),
        });
    }

    // 5. DirectX 9 / 8
    if let Some(dll) = imported_dlls
        .iter()
        .find(|d| d.contains("d3d9") || d.contains("d3d8"))
    {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::DirectX9OrOlder,
            engine: TargetEngine::WineStaging,
            detected_dll: Some(dll.clone()),
        });
    }

    // 6. DirectDraw / DirectX 1-7 (ddraw.dll)
    // Routes to KosmicKrisp if D7VK is installed, otherwise falls back to WineStaging (WineD3D).
    if let Some(dll) = imported_dlls.iter().find(|d| d.contains("ddraw")) {
        let engine = if crate::d7vk::is_d7vk_installed() {
            TargetEngine::KosmicKrisp
        } else {
            TargetEngine::WineStaging
        };
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::DirectX7OrOlder,
            engine,
            detected_dll: Some(dll.clone()),
        });
    }

    // 6. Vulkan -> Route to Mesa KosmicKrisp (Vulkan 1.4 conformant driver on Metal 4)
    if imported_dlls.iter().any(|d| d.contains("vulkan-1")) {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::Vulkan,
            engine: TargetEngine::KosmicKrisp,
            detected_dll: Some("vulkan-1.dll".into()),
        });
    }

    // 7. OpenGL
    if imported_dlls.iter().any(|d| d.contains("opengl32")) {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::OpenGL,
            engine: TargetEngine::WineStaging,
            detected_dll: Some("opengl32.dll".into()),
        });
    }

    Ok(GraphicsApiInfo {
        api: GraphicsApi::Unknown,
        engine: TargetEngine::Gptk,
        detected_dll: None,
    })
}

fn scan_directory_for_graphics_api(dir: &Path, depth: u32) -> Option<GraphicsApiInfo> {
    if depth == 0 {
        return None;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("exe") {
                        let name = p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        if !name.contains("unins")
                            && !name.contains("crash")
                            && !name.contains("report")
                        {
                            if let Ok(info) = inspect_pe_file(&p) {
                                if info.api != GraphicsApi::Unknown {
                                    return Some(info);
                                }
                            }
                        }
                    }
                }
            } else if p.is_dir() {
                let dname = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if dname == "binaries" || dname == "bin" || dname == "x64" || dname == "win64" {
                    if let Some(info) = scan_directory_for_graphics_api(&p, depth - 1) {
                        return Some(info);
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detection_priorities() {
        // Simulated binary containing d3d12.dll
        let bytes_dx12 = b"MZ\x90\x00...SomeHeader...d3d12.dll\x00kernel32.dll\x00";
        let info = inspect_pe_bytes(bytes_dx12).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX12);
        assert_eq!(info.engine, TargetEngine::Gptk);

        // Simulated binary containing d3d11.dll
        let bytes_dx11 = b"MZ\x90\x00...SomeHeader...d3d11.dll\x00dxgi.dll\x00";
        let info = inspect_pe_bytes(bytes_dx11).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX11);
        assert_eq!(info.engine, TargetEngine::Gptk);

        // Simulated binary containing d3d10.dll
        let bytes_dx10 = b"MZ\x90\x00...SomeHeader...d3d10.dll\x00user32.dll\x00";
        let info = inspect_pe_bytes(bytes_dx10).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX10);
        assert_eq!(info.engine, TargetEngine::WineStaging);

        // Simulated binary containing d3d9.dll
        let bytes_dx9 = b"MZ\x90\x00...SomeHeader...d3d9.dll\x00user32.dll\x00";
        let info = inspect_pe_bytes(bytes_dx9).unwrap();
        assert_eq!(info.api, GraphicsApi::DirectX9OrOlder);
        assert_eq!(info.engine, TargetEngine::WineStaging);

        // Simulated binary containing vulkan-1.dll
        let bytes_vk = b"MZ\x90\x00...SomeHeader...vulkan-1.dll\x00kernel32.dll\x00";
        let info = inspect_pe_bytes(bytes_vk).unwrap();
        assert_eq!(info.api, GraphicsApi::Vulkan);
        assert_eq!(info.engine, TargetEngine::KosmicKrisp);

        // Simulated binary containing opengl32.dll
        let bytes_gl = b"MZ\x90\x00...SomeHeader...opengl32.dll\x00gdi32.dll\x00";
        let info = inspect_pe_bytes(bytes_gl).unwrap();
        assert_eq!(info.api, GraphicsApi::OpenGL);
        assert_eq!(info.engine, TargetEngine::WineStaging);

        // Simulated binary containing ddraw.dll
        let bytes_ddraw = b"MZ\x90\x00...SomeHeader...ddraw.dll\x00kernel32.dll\x00";
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
            "Wine + Mesa KosmicKrisp Vulkan"
        );
        assert_eq!(
            TargetEngine::WineStaging.display_name(),
            "Wine + WineD3D OpenGL"
        );
    }

    #[test]
    fn test_wine_engine_routing() {
        let info_dx11 = GraphicsApiInfo {
            api: GraphicsApi::DirectX11,
            engine: TargetEngine::Gptk,
            detected_dll: Some("d3d11.dll".into()),
        };
        // Auto runner is for Wine only: DX11 routes to KosmicKrisp if installed or WineStaging
        let wine_eng = info_dx11.wine_engine();
        assert!(wine_eng == TargetEngine::KosmicKrisp || wine_eng == TargetEngine::WineStaging);

        let info_dx9 = GraphicsApiInfo {
            api: GraphicsApi::DirectX9OrOlder,
            engine: TargetEngine::WineStaging,
            detected_dll: Some("d3d9.dll".into()),
        };
        assert_eq!(info_dx9.wine_engine(), TargetEngine::WineStaging);
    }
}
