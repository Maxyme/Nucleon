# Nucleon

**Nucleon** is a high-performance Windows game translation and compatibility layer for macOS, engineered in Rust. It integrates Apple's **Game Porting Toolkit (GPTK 4)** and **Wine** directly into macOS and Steam, delivering near-native DirectX 11/12 gaming on Apple Silicon.

Originally conceived from multi-language scripts and shims, Nucleon is a ground-up consolidation into a unified, memory-safe Rust workspace.

---

## Architecture Overview

Nucleon consolidates all compatibility tooling, process orchestration, binary hooking, and window presentation validation into a single Cargo workspace:

```
crates/
├── dobby-sys/         # Rust FFI wrapper for Dobby inline hook engine
├── overlay-shim/      # Metal / QuartzCore frame presentation & HUD shim
├── nucleon-core/      # Shared runtime, prefix config, VDF generator, manifest fetcher, CG validator
├── nucleon-runner/    # Native Steam compatibility tool runner & process supervisor
├── nucleon-hook/      # Universal Mach-O dylib injected into steam_osx (Dobby hooks & WebUI patching)
└── nucleon-cli/       # Primary user CLI (nucleon setup, status, launch, validate, steam)
```

### Core Components

- **`nucleon-cli` (`nucleon`)**: The unified CLI management tool.
  - Automates runner discovery, staging, Valve bridge deployment, and Steam client patching.
  - Provides instant status reports and diagnostic introspection.
  - Launches Steam titles with optional Apple Metal Performance HUD overlays.
  - Zero-screen-capture frame presentation validation using native WindowServer metadata.
- **`nucleon-runner`**: Compiled replacement for legacy shell scripts.
  - Native supervision of Wine processes, handling child life-cycles, foreground window activation via Cocoa, and clean signal termination (`SIGINT`/`SIGTERM`).
  - Seamlessly handles Steam verbs (`run`, `waitforexitandrun`, `check-app-compatibility`) and intercepts legacy helper binaries (`iscriptevaluator.exe`).
  - Configures optimal Wine environment parameters (`D3DM_MTL4=1`, `WINEMSYNC=1`, `WINEDLLOVERRIDES`).
- **`nucleon-hook` (`libnucleon.dylib`)**: Universal Mach-O library injected into `steam_osx`.
  - Employs Dobby inline hooks to override Steam client compatibility checks (`CCompatManager::Init`, `CCompatManager::BIsEnabled`).
  - Interposes file descriptors (`open`/`openat`) to patch Steam CEF WebUI chunks in memory, exposing the native Compatibility settings tab.
  - Built as a universal fat binary (`arm64` + `x86_64`) with ad-hoc code signatures to run under both native ARM64 and Rosetta execution.
- **`overlay-shim`**: Native Metal translation layer hooking `[CAMetalLayer nextDrawable]` to ensure stutter-free presentation and HUD telemetry.
- **`nucleon-core`**: Shared models, Wine prefix initialization, registry overrides (`OpenGLSurfaceMode=behind`), and Valve package manifest unpacking.

---

## Runtime & Engine Compatibility

- **Apple Game Porting Toolkit (GPTK 4 / GPTK 2)**: Fully supported and recommended for DirectX 11 and DirectX 12 titles via `D3DMetal.framework`.
- **Wine-Staging**: Supported for general DirectX 9/11 and Vulkan titles.
- **Architecture**: Native Apple Silicon 64-bit translation. *Note: Experimental 32-bit support exists, though it has not been validated across titles.*

---

## Getting Started

### Prerequisites

- macOS 14 (Sonoma) or macOS 15 (Sequoia) on Apple Silicon.
- [Rust](https://rustup.rs/) (1.80+ recommended).
- Xcode Command Line Tools (`xcode-select --install`).
- Apple Game Porting Toolkit 4 DMG mounted or installed, or Wine-Staging.
- Steam for macOS installed in `/Applications/Steam.app`.

### Building from Source

To build all release binaries and the universal hook library:

```bash
# Add x86_64 target for universal fat dylib slicing
rustup target add x86_64-apple-darwin

# Build release binaries and universal dylib
make build
```

### Initial Setup

Run the automated setup command to stage bridge packages, register the Steam compatibility tool, and patch Steam:

```bash
./target/release/nucleon setup
```

### Inspecting Status

Check the status of your installation, active runner, and Steam integration:

```bash
./target/release/nucleon status
```

### Launching Games

Launch any installed Steam game by its Application ID:

```bash
# Launch game normally
./target/release/nucleon launch <AppID>

# Launch with Apple Metal Performance HUD enabled
./target/release/nucleon launch <AppID> --hud
```

### Validating Window Presentation

Nucleon adheres to a strict zero-screen-capture policy. To verify that a game's window is actively drawing on screen, inspect WindowServer layer metadata directly:

```bash
./target/release/nucleon validate <AppID>
```

---

## Roadmap

See [TODO.md](TODO.md) for upcoming milestones, including:
- **Host Application & GUI Consolidation**: A standalone native macOS GUI (SwiftUI / Tauri) for visual game library management, prefix configuration, and graphics profiling.
- Automated CEF WebUI chunk patch resolution for future Steam client updates.
- Isolated per-AppID prefix profiles.

---

## License

MIT OR Apache-2.0.
