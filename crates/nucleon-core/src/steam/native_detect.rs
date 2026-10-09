use crate::paths;
use std::collections::HashSet;
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
        if let Ok(content) = fs::read_to_string(vdf_path) {
            if let Ok(partial) = keyvalues_parser::parse(&content) {
                let vdf = keyvalues_parser::Vdf::from(partial);
                if let keyvalues_parser::Value::Obj(ref root_obj) = vdf.value {
                    for folder_vals in root_obj.values() {
                        for folder_val in folder_vals {
                            if let keyvalues_parser::Value::Obj(ref folder_obj) = folder_val {
                                if let Some(path_str) = folder_obj
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
        }
    }

    dirs
}

/// Scans the given Steam `steamapps` directories for installed games and returns
/// the AppIDs of all detected native macOS games.
pub fn find_native_mac_appids_in(steamapps_dirs: &[PathBuf]) -> HashSet<String> {
    let mut native_appids = HashSet::new();

    for steamapps in steamapps_dirs {
        if !steamapps.is_dir() {
            continue;
        }

        let entries = match fs::read_dir(steamapps) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let p = entry.path();
            let Some(file_name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };

            if file_name.starts_with("appmanifest_") && file_name.ends_with(".acf") {
                let id = file_name
                    .trim_start_matches("appmanifest_")
                    .trim_end_matches(".acf");
                if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }

                if let Ok(acf_content) = fs::read_to_string(&p) {
                    let installdir =
                        keyvalues_parser::parse(&acf_content)
                            .ok()
                            .and_then(|partial| {
                                let vdf = keyvalues_parser::Vdf::from(partial);
                                if let keyvalues_parser::Value::Obj(ref obj) = vdf.value {
                                    obj.get("installdir")
                                        .and_then(|v| v.first())
                                        .and_then(|v| v.get_str())
                                        .map(|s| s.to_string())
                                } else {
                                    None
                                }
                            });

                    if let Some(dir_name) = installdir {
                        let game_dir = steamapps.join("common").join(dir_name);
                        if is_directory_native_mac(&game_dir, 3) {
                            native_appids.insert(id.to_string());
                        }
                    }
                }
            }
        }
    }

    native_appids
}

/// Scans all local Steam library folders for installed games and returns
/// the AppIDs of all detected native macOS games.
pub fn find_native_mac_appids() -> HashSet<String> {
    let dirs = find_steamapps_dirs();
    find_native_mac_appids_in(&dirs)
}

/// Inspects Steam's local appcache/appinfo.vdf to determine if an AppID supports macOS natively.
pub fn is_app_supported_on_macos_in_appinfo(appid: u32) -> Option<bool> {
    let appinfo_path = paths::steam_data_dir().join("appcache/appinfo.vdf");
    if !appinfo_path.exists() {
        return None;
    }
    let data = fs::read(&appinfo_path).ok()?;
    let needle = appid.to_le_bytes();
    let idx = memchr::memmem::find(&data, &needle)?;
    let size = if idx + 8 <= data.len() {
        u32::from_le_bytes(data[idx + 4..idx + 8].try_into().ok()?) as usize
    } else {
        4096
    };
    let scan_len = std::cmp::min(data.len() - idx, std::cmp::max(size, 4096));
    let chunk = &data[idx..idx + scan_len];
    Some(memchr::memmem::find(chunk, b"macos").is_some())
}

/// Checks if a Steam game with the given AppID is a native macOS game,
/// checking installed game files first and falling back to appcache/appinfo.vdf.
pub fn is_app_native_mac(appid: &str) -> bool {
    let dirs = find_steamapps_dirs();
    for steamapps in &dirs {
        let manifest_path = steamapps.join(format!("appmanifest_{appid}.acf"));
        if let Ok(content) = fs::read_to_string(&manifest_path) {
            let installdir = keyvalues_parser::parse(&content).ok().and_then(|partial| {
                let vdf = keyvalues_parser::Vdf::from(partial);
                if let keyvalues_parser::Value::Obj(ref obj) = vdf.value {
                    obj.get("installdir")
                        .and_then(|v| v.first())
                        .and_then(|v| v.get_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            });

            if let Some(dir_name) = installdir {
                let game_dir = steamapps.join("common").join(dir_name);
                return is_directory_native_mac(&game_dir, 3);
            }
        }
    }

    if let Ok(num) = appid.parse::<u32>() {
        if let Some(is_native) = is_app_supported_on_macos_in_appinfo(num) {
            return is_native;
        }
    }

    false
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

    #[test]
    fn test_find_native_mac_appids_in() {
        let temp = tempfile::tempdir().unwrap();
        let steamapps = temp.path().join("steamapps");
        let common = steamapps.join("common");
        fs::create_dir_all(&common).unwrap();

        // App 1001: Native game with .app bundle
        let native_dir = common.join("NativeGame");
        fs::create_dir_all(native_dir.join("NativeGame.app")).unwrap();
        let manifest_1001 = r#""AppState"
{
	"appid"		"1001"
	"name"		"Native Game"
	"installdir"		"NativeGame"
}
"#;
        fs::write(steamapps.join("appmanifest_1001.acf"), manifest_1001).unwrap();

        // App 1002: Native game with Mach-O executable
        let macho_dir = common.join("MachoGame");
        fs::create_dir_all(&macho_dir).unwrap();
        fs::write(
            macho_dir.join("macho_bin"),
            [0xcf, 0xfa, 0xed, 0xfe, 0x00, 0x00, 0x00, 0x00],
        )
        .unwrap();
        let manifest_1002 = r#""AppState"
{
	"appid"		"1002"
	"name"		"Macho Game"
	"installdir"		"MachoGame"
}
"#;
        fs::write(steamapps.join("appmanifest_1002.acf"), manifest_1002).unwrap();

        // App 2001: Windows-only game
        let win_dir = common.join("WindowsGame");
        fs::create_dir_all(&win_dir).unwrap();
        fs::write(win_dir.join("game.exe"), [0x4d, 0x5a, 0x90, 0x00]).unwrap();
        let manifest_2001 = r#""AppState"
{
	"appid"		"2001"
	"name"		"Windows Game"
	"installdir"		"WindowsGame"
}
"#;
        fs::write(steamapps.join("appmanifest_2001.acf"), manifest_2001).unwrap();

        // App 3001: Missing game folder
        let manifest_3001 = r#""AppState"
{
	"appid"		"3001"
	"name"		"Missing Game"
	"installdir"		"NonExistent"
}
"#;
        fs::write(steamapps.join("appmanifest_3001.acf"), manifest_3001).unwrap();

        let detected = find_native_mac_appids_in(&[steamapps]);
        assert!(detected.contains("1001"));
        assert!(detected.contains("1002"));
        assert!(!detected.contains("2001"));
        assert!(!detected.contains("3001"));
    }

    #[test]
    fn test_is_app_supported_on_macos_in_appinfo_when_present() {
        // Devil May Cry 5 (601150) is Windows-only; Stellaris (281990) has native macOS
        if paths::steam_data_dir()
            .join("appcache/appinfo.vdf")
            .exists()
        {
            assert_eq!(is_app_supported_on_macos_in_appinfo(601150), Some(false));
            assert_eq!(is_app_supported_on_macos_in_appinfo(281990), Some(true));
        }
    }
}
