use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest_dir.join("src/overlay_shim.m");
    let target_dylib = out_dir.join("overlay-shim.dylib");

    println!("cargo:rerun-if-changed={}", src.display());

    let status = Command::new("clang")
        .args([
            "-arch",
            "arm64",
            "-arch",
            "x86_64",
            "-mmacosx-version-min=15.0",
            "-dynamiclib",
            "-O2",
            "-Wall",
            "-Wextra",
            "-install_name",
            "@rpath/overlay-shim.dylib",
            "-framework",
            "Metal",
            "-framework",
            "QuartzCore",
            "-framework",
            "CoreGraphics",
            "-framework",
            "CoreFoundation",
            "-framework",
            "AppKit",
            "-o",
            target_dylib.to_str().unwrap(),
            src.to_str().unwrap(),
        ])
        .status();

    if let Ok(st) = status {
        if !st.success() {
            // Fallback to native arch only if universal fails
            let fallback_status = Command::new("clang")
                .args([
                    "-arch",
                    "arm64",
                    "-mmacosx-version-min=15.0",
                    "-dynamiclib",
                    "-O2",
                    "-Wall",
                    "-Wextra",
                    "-install_name",
                    "@rpath/overlay-shim.dylib",
                    "-framework",
                    "Metal",
                    "-framework",
                    "QuartzCore",
                    "-framework",
                    "CoreGraphics",
                    "-framework",
                    "CoreFoundation",
                    "-framework",
                    "AppKit",
                    "-o",
                    target_dylib.to_str().unwrap(),
                    src.to_str().unwrap(),
                ])
                .status();
            assert!(fallback_status.map(|s| s.success()).unwrap_or(false));
        }
    }

    // Ad-hoc sign
    let _ = Command::new("codesign")
        .args(["-fs", "-", target_dylib.to_str().unwrap()])
        .status();

    // Export output dylib path
    println!(
        "cargo:rustc-env=OVERLAY_SHIM_DYLIB={}",
        target_dylib.display()
    );
}
