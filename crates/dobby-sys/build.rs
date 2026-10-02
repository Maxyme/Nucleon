use std::env;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let arch = if target.starts_with("x86_64") {
        "x86_64"
    } else {
        "arm64"
    };

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor_dobby = manifest_dir.join("../../vendor/dobby");

    let dst = cmake::Config::new(&vendor_dobby)
        .define("CMAKE_OSX_ARCHITECTURES", arch)
        .define("CMAKE_OSX_DEPLOYMENT_TARGET", "15.0")
        .define("DOBBY_DEBUG", "OFF")
        .generator("Unix Makefiles")
        .build_target("dobby_static")
        .build();

    let build_dir = dst.join("build");
    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!("cargo:rustc-link-lib=static=dobby");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=c++");

    println!("cargo:rerun-if-changed={}", vendor_dobby.display());
}
