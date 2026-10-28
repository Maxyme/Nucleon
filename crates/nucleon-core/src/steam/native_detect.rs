use crate::paths;
use std::fs;
use std::path::{Path, PathBuf};

/// Checks if a file is a Mach-O binary by reading its 4-byte magic number.
pub fn is_macho_binary(path: &Path) -> bool {
    if let Ok(mut f) = fs::File::open(path) {
        use std::io::Read;
        let mut magic = [0u8; 4];
        if f.read_exact(&mut magic).is_ok() {
            return matches!(
                &magic,
                [0xfe, 0xed, 0xfa, 0xce]
                    | [0xfe, 0xed, 0xfa, 0xcf]
                    | [0xce, 0xfa, 0xed, 0xfe]
                    | [0xcf, 0xfa, 0xed, 0xfe]
                    | [0xca, 0xfe, 0xba, 0xbe]
                    | [0xbe, 0xba, 0xfe, 0xca]
                    | [0xca, 0xfe, 0xba, 0xbf] // Fat64
                    | [0xbf, 0xba, 0xfe, 0xca] // Fat64 reverse endian
            );
        }
    }
    false
}

/// Recursively checks if a game directory contains a macOS .app bundle or Mach-O executable.
pub fn is_directory_native_mac(dir: &Path, max_depth: u32) -> bool {
    if !dir.is_dir() || max_depth == 0 {
        return false;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.extension().and_then(|s| s.to_str()) == Some("app") {
                    return true;
                }
                if is_directory_native_mac(&p, max_depth - 1) {
                    return true;
                }
            } else if p.is_file() && is_macho_binary(&p) {
                return true;
            }
        }
    }
    false
}

/// Discovers all Steam library `steamapps` directories (primary and external libraries).
pub fn find_steamapps_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let primary = paths::steam_data_dir().join("steamapps");
    if primary.is_dir() {
        dirs.push(primary);
    }

    let libraryfolders_candidates = [
        paths::steam_data_dir().join("steamapps/libraryfolders.vdf"),
        paths::steam_data_dir().join("config/libraryfolders.vdf"),
    ];

    for vdf_path in &libraryfolders_candidates {
        if let Ok(content) = fs::read_to_string(vdf_path)
            && let Ok(partial) = keyvalues_parser::parse(&content)
        {
            let vdf = keyvalues_parser::Vdf::from(partial);
            if let keyvalues_parser::Value::Obj(ref root_obj) = vdf.value {
                for folder_vals in root_obj.values() {
                    for folder_val in folder_vals {
                        if let keyvalues_parser::Value::Obj(folder_obj) = folder_val
                            && let Some(path_str) = folder_obj
                                .get("path")
                                .and_then(|v| v.first())
                                .and_then(|v| v.get_str())
                        {
                            let base = PathBuf::from(path_str);
                            let candidate = if base.ends_with("steamapps") {
                                base
                            } else {
                                base.join("steamapps")
                            };
                            if candidate.is_dir() && !dirs.contains(&candidate) {
                                dirs.push(candidate);
                            }
                        }
                    }
                }
            }
        }
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_macho_binary() {
        let temp = tempfile::tempdir().unwrap();

        // Valid Mach-O 64-bit LE
        let macho_path = temp.path().join("game_binary");
        fs::write(
            &macho_path,
            [0xcf, 0xfa, 0xed, 0xfe, 0x01, 0x00, 0x00, 0x00],
        )
        .unwrap();
        assert!(is_macho_binary(&macho_path));

        // Valid Mach-O Fat Universal binary
        let fat_path = temp.path().join("fat_binary");
        fs::write(&fat_path, [0xca, 0xfe, 0xba, 0xbe, 0x00, 0x00, 0x00, 0x02]).unwrap();
        assert!(is_macho_binary(&fat_path));

        // Windows PE binary (MZ magic)
        let pe_path = temp.path().join("game.exe");
        fs::write(&pe_path, [0x4d, 0x5a, 0x90, 0x00]).unwrap();
        assert!(!is_macho_binary(&pe_path));

        // Non-existent path
        assert!(!is_macho_binary(&temp.path().join("nonexistent")));
    }

    #[test]
    fn test_is_directory_native_mac() {
        let temp = tempfile::tempdir().unwrap();

        // Native app bundle inside game dir
        let game_with_app = temp.path().join("GameWithApp");
        fs::create_dir_all(game_with_app.join("Game.app/Contents/MacOS")).unwrap();
        assert!(is_directory_native_mac(&game_with_app, 3));

        // Native Mach-O binary inside game dir
        let game_with_macho = temp.path().join("GameWithMacho");
        fs::create_dir_all(&game_with_macho).unwrap();
        fs::write(
            game_with_macho.join("game_runner"),
            [0xfe, 0xed, 0xfa, 0xcf, 0x00, 0x00, 0x00, 0x00],
        )
        .unwrap();
        assert!(is_directory_native_mac(&game_with_macho, 3));

        // Windows-only game directory
        let game_windows = temp.path().join("WindowsGame");
        fs::create_dir_all(&game_windows).unwrap();
        fs::write(game_windows.join("game.exe"), [0x4d, 0x5a, 0x90, 0x00]).unwrap();
        fs::write(game_windows.join("game.dll"), [0x4d, 0x5a, 0x90, 0x00]).unwrap();
        assert!(!is_directory_native_mac(&game_windows, 3));
    }
}
