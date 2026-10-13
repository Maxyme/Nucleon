# Nucleon task runner
set shell := ["bash", "-uc"]

target_dir := "target/release"

# Default recipe: build all binaries and universal hook dylib
default: build

# Build all release binaries and universal hook dylib
build: build-hook build-bins

# Build CLI release binary (single binary embedding runner)
build-bins:
	cargo build --release -p nucleon-cli
	ln -sf nucleon {{target_dir}}/nucleon-runner

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

# Format all code across the workspace
fmt:
    cargo fmt --all

# Lint checks
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Format code and auto-apply clippy fixes
fix:
    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features --fix --allow-dirty --allow-staged

# Run workspace unit and integration tests
test:
    cargo test --workspace

# Complete validation: formatting check, strict clippy lints, and test suite
check: lint test
    @echo "✓ All formatting, clippy lints, and tests passed cleanly!"

# Build and package the release bundle (thin arm64) for Apple Silicon
package:
	@echo "==> Building thin arm64 release binary and hook..."
	cargo build --release --target aarch64-apple-darwin -p nucleon-cli -p nucleon-hook
	@echo "==> Applying ad-hoc codesignatures..."
	codesign -fs - target/aarch64-apple-darwin/release/nucleon
	codesign -fs - target/aarch64-apple-darwin/release/libnucleon.dylib
	@echo "==> Assembling release bundle directory..."
	rm -rf target/nucleon-macos-arm64 target/nucleon-macos-arm64.tar.gz
	mkdir -p target/nucleon-macos-arm64/assets
	cp target/aarch64-apple-darwin/release/nucleon target/nucleon-macos-arm64/
	ln -sf nucleon target/nucleon-macos-arm64/nucleon-runner
	cp target/aarch64-apple-darwin/release/libnucleon.dylib target/nucleon-macos-arm64/
	cp assets/bridge-manifest.json target/nucleon-macos-arm64/assets/
	cp -R assets/steam-contracts target/nucleon-macos-arm64/assets/
	cp README.md ARCHITECTURE.md SETUP.md TROUBLESHOOTING.md ROADMAP.md LICENSE NOTICE.md target/nucleon-macos-arm64/
	git archive --format=tar.gz --prefix=nucleon-source/ HEAD -o target/nucleon-source.tar.gz
	tar -czvf target/nucleon-macos-arm64.tar.gz -C target/nucleon-macos-arm64 .
	@echo "✓ Release bundle created at target/nucleon-macos-arm64.tar.gz"
