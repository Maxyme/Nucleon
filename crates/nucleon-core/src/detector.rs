use std::fs;
use std::path::Path;
use object::Object;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetEngine {
    Gptk,
    WineStaging,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphicsApi {
    DirectX12,
    DirectX11,
    DirectX10,
    DirectX9OrOlder,
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
            "d3d12.dll", "d3d11.dll", "dxgi.dll", "d3d10.dll", "d3d10_1.dll",
            "d3d10core.dll", "d3d9.dll", "d3d8.dll", "ddraw.dll", "vulkan-1.dll", "opengl32.dll"
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

    // 5. DirectX 9 or older
    if let Some(dll) = imported_dlls.iter().find(|d| d.contains("d3d9") || d.contains("d3d8") || d.contains("ddraw")) {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::DirectX9OrOlder,
            engine: TargetEngine::WineStaging,
            detected_dll: Some(dll.clone()),
        });
    }

    // 6. Vulkan
    if imported_dlls.iter().any(|d| d.contains("vulkan-1")) {
        return Ok(GraphicsApiInfo {
            api: GraphicsApi::Vulkan,
            engine: TargetEngine::WineStaging,
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
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
                        if !name.contains("unins") && !name.contains("crash") && !name.contains("report") {
                            if let Ok(info) = inspect_pe_file(&p) {
                                if info.api != GraphicsApi::Unknown {
                                    return Some(info);
                                }
                            }
                        }
                    }
                }
            } else if p.is_dir() {
                let dname = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
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

        // Simulated binary containing opengl32.dll
        let bytes_gl = b"MZ\x90\x00...SomeHeader...opengl32.dll\x00gdi32.dll\x00";
        let info = inspect_pe_bytes(bytes_gl).unwrap();
        assert_eq!(info.api, GraphicsApi::OpenGL);
        assert_eq!(info.engine, TargetEngine::WineStaging);
    }
}

