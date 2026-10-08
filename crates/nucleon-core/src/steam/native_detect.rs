use std::fs;
use std::path::Path;

/// Known native macOS Steam AppIDs that should run natively without compatibility tools.
pub const KNOWN_NATIVE_MAC_APPIDS: &[&str] = &[
    "281990",  // Stellaris
    "421020",  // DiRT 4
    "1091500", // Cyberpunk 2077
    "391220",  // Rise of the Tomb Raider
    "2366970", // Arco
];

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
