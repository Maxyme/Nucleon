fn main() {
    // Track directory membership as well as existing file contents on stable Rust.
    println!("cargo:rerun-if-changed=../../assets");
    println!("cargo:rerun-if-changed=tests/fixtures");
}
