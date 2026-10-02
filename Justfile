# Nucleon task runner
set shell := ["bash", "-uc"]

target_dir := "target/release"

# Default recipe: build all binaries and universal hook dylib
default: build

# Build all release binaries and universal hook dylib
build: build-hook build-bins

# Build CLI and runner release binaries
build-bins:
    cargo build --release -p nucleon-cli -p nucleon-runner

# Build universal Mach-O hook dylib (arm64 + x86_64)
build-hook:
    cargo build --release --target aarch64-apple-darwin -p nucleon-hook
    cargo build --release --target x86_64-apple-darwin -p nucleon-hook
    mkdir -p {{target_dir}}
    lipo -create -output {{target_dir}}/libnucleon.dylib \
        target/aarch64-apple-darwin/release/libnucleon.dylib \
        target/x86_64-apple-darwin/release/libnucleon.dylib
    cp {{target_dir}}/libnucleon.dylib {{target_dir}}/nucleon.dylib
    codesign -fs - {{target_dir}}/libnucleon.dylib
    codesign -fs - {{target_dir}}/nucleon.dylib

# Run automated Nucleon setup
setup: build
    ./{{target_dir}}/nucleon setup

# Inspect Nucleon runtime status
status:
    ./{{target_dir}}/nucleon status

# Run workspace unit and integration tests
test:
    cargo test --workspace

# Run clippy across all workspace crates and targets
lint:
    cargo clippy --workspace --all-targets

# Clean build artifacts
clean:
    cargo clean
