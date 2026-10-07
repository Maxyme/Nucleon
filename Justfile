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

# Stage Valve bridge from an extracted directory: just stage-bridge /path/to/extracted
stage-bridge path: build
    ./{{target_dir}}/nucleon setup --bridge-path {{path}}

# Remove Nucleon from Steam UI, compatibility tools, and game mappings
unregister: build
    ./{{target_dir}}/nucleon unregister


# Inspect Nucleon runtime status
status:
    ./{{target_dir}}/nucleon status

# Inspect Steam Update Guard status
guard-status:
    ./{{target_dir}}/nucleon guard status

# Run Steam Update Guard self-healing verification
guard-run:
    ./{{target_dir}}/nucleon guard run

# Install and activate Background Steam Update Guard LaunchAgent
guard-install:
    ./{{target_dir}}/nucleon guard install

# Check code formatting without making changes
fmt-check:
    @echo "==> [1/3] Checking Rust formatting (cargo fmt --check)..."
    cargo fmt --all -- --check

# Format all code across the workspace
fmt:
    @echo "==> Formatting workspace with cargo fmt..."
    cargo fmt --all
    @echo "✓ Formatting completed."

# Run clippy across all workspace crates and targets
lint:
    @echo "==> [2/3] Checking Clippy lints (cargo clippy -D warnings)..."
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Format code and auto-apply clippy fixes
fix:
    @echo "==> Formatting workspace with cargo fmt..."
    cargo fmt --all
    @echo "==> Applying clippy auto-fixes if applicable..."
    cargo clippy --workspace --all-targets --all-features --fix --allow-dirty --allow-staged
    @echo "✓ Formatting and clippy fixes completed."

# Run workspace unit and integration tests
test:
    @echo "==> [3/3] Running tests (cargo test)..."
    cargo test --workspace

# Complete validation: formatting check, strict clippy lints, and test suite
check: fmt-check lint test
    @echo "✓ All formatting, clippy lints, and tests passed cleanly!"
