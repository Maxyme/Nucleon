# Nucleon

**Nucleon** is a high-performance Windows game translation and compatibility layer for macOS, engineered in Rust. It integrates Apple's **Game Porting Toolkit (GPTK 4)** and **Wine** directly into macOS and Steam, delivering near-native DirectX 11/12 gaming on Apple Silicon.

Originally conceived from multi-language scripts and shims, Nucleon is a ground-up consolidation into a unified, memory-safe Rust workspace.

---

## Architecture Overview

Nucleon consolidates all compatibility tooling, process orchestration, binary hooking, and window presentation validation into a single Cargo workspace:

```
crates/
├── overlay-shim/      # Metal / QuartzCore frame presentation & HUD shim
├── nucleon-core/      # Shared runtime, prefix config, VDF generator, manifest fetcher, CG validator
├── nucleon-runner/    # Native Steam compatibility tool runner & process supervisor
├── nucleon-hook/      # Universal Mach-O dylib injected into steam_osx (Frida Gum hooks & WebUI patching)
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
  - Powered by **Frida Gum** (`frida-gum`) for robust inline function interception on macOS Apple Silicon and x86_64 (`CCompatManager::Init`, `CCompatManager::BIsEnabled`).
  - Interposes file descriptors (`open`/`openat`) to patch Steam CEF WebUI chunks in memory (enables `.exe` selection in non-Steam game dialogs and displays Windows game compatibility notices).
  - Built as a universal fat binary (`arm64` + `x86_64`) with ad-hoc code signatures to run under both native ARM64 and Rosetta execution.
- **`overlay-shim`**: Native Metal translation layer hooking `[CAMetalLayer nextDrawable]` to ensure stutter-free presentation and HUD telemetry.
- **`nucleon-core`**: Shared models, Wine prefix initialization, registry overrides (`OpenGLSurfaceMode=behind`), and Valve package manifest unpacking.

---

## Tri-Engine Architecture & Automatic Graphics Routing

Nucleon features an intelligent **Tri-Engine Router** that inspects target Windows binaries and automatically dispatches to the optimal translation engine:

| Graphics API | Imported DLLs | Dispatched Engine | Pipeline Characteristics |
| :--- | :--- | :--- | :--- |
| **DirectX 12** | `d3d12.dll`, `dxgi.dll` | **Apple GPTK 4** | Native `D3DMetal` HLSL-to-MSL translation, Metal 4, FastSync |
| **DirectX 11** | `d3d11.dll`, `dxgi.dll` | **Apple GPTK 4** | Native `D3DMetal` Direct3D 11 to Metal hardware acceleration |
| **Vulkan 1.3 / 1.4** | `vulkan-1.dll` | **Mesa KosmicKrisp** | Khronos-conformant Vulkan 1.4 driver on Metal 4 (Mesa NIR compiler) |
| **DirectX 10** | `d3d10.dll`, `d3d10_1.dll`, `d3d10core.dll` | **Wine-Staging** / **KosmicKrisp** | Full `wined3d` / DXVK pipeline (D3DMetal lacks DX10 support) |
| **DirectX 9 & Older** | `d3d9.dll`, `d3d8.dll`, `ddraw.dll` | **Wine-Staging** / **KosmicKrisp** | Upstream Wine legacy pipeline or DXVK on Vulkan 1.4 |
| **OpenGL** | `opengl32.dll` | **Wine-Staging** / **KosmicKrisp** | Native macOS OpenGL or Mesa Zink-on-KosmicKrisp (OpenGL 4.6) |

### How It Works
1. **PE Import Table Analysis**: When launching a title, `nucleon-runner` parses the Windows Portable Executable (PE) headers and Import Address Table (IAT) using the Rust `object` engine.
2. **Dynamic Dispatch**:
   - DX11/12 titles route to Apple GPTK with `D3DM_MTL4=1`, `WINEMSYNC=1`, and Apple's `D3DMetal.framework`.
   - Native Vulkan titles (`vulkan-1.dll`) automatically route to Mesa KosmicKrisp (`VK_DRIVER_FILES=.../libkosmickrisp_icd.json`).
   - DX9/10 titles route to Wine-Staging or KosmicKrisp with DXVK overrides.
   - If Wine-Staging or KosmicKrisp is not installed, Nucleon gracefully falls back to the available GPTK runner with legacy DLL configurations.
3. **Manual Overrides**: You can override engine selection at any time using `--engine <gptk|kosmickrisp|staging>` or the `NUCLEON_ENGINE` environment variable.

---

## Runtime & Engine Compatibility

- **Apple Game Porting Toolkit (GPTK 4 / GPTK 2)**: Fully supported and recommended for DirectX 11 and DirectX 12 titles via `D3DMetal.framework`.
- **Mesa KosmicKrisp**: Full Vulkan 1.4 conformant driver implemented on Metal 4 for Apple Silicon (macOS 26+). Recommended for native Vulkan titles and open-source Direct3D via DXVK and VKD3D-Proton.
- **VKD3D-Proton (Direct3D 12 -> Vulkan 1.4)**: Translates Direct3D 12 calls to Vulkan 1.4 when running with KosmicKrisp. Binaries are never tracked in git; you can extract official releases and configure Nucleon via `nucleon vkd3d set-path <dir>`, `nucleon setup --vkd3d-path <dir>`, or `VKD3D_PROTON_PATH`.
- **Wine-Staging**: Supported for DirectX 9, DirectX 10, and general legacy Windows applications (`brew install --cask wine-staging`).
- **Architecture**: Native Apple Silicon 64-bit translation. *Note: Experimental 32-bit support exists, though it has not been validated across titles.*

---

## Getting Started

### Prerequisites

- macOS 14 (Sonoma) or macOS 15 (Sequoia) on Apple Silicon.
- [Rust](https://rustup.rs/) (1.80+ recommended).
- [just](https://github.com/casey/just) command runner (`brew install just`).
- Xcode Command Line Tools (`xcode-select --install`).
- Apple Game Porting Toolkit 4 DMG mounted or installed, or Wine-Staging.
- Steam for macOS installed in `/Applications/Steam.app`.

### Building from Source

To build all release binaries and the universal hook library:

```bash
# Add x86_64 target for universal fat dylib slicing
rustup target add x86_64-apple-darwin

# Build release binaries and universal dylib
just build
```

### Initial Setup

Run the automated setup command to stage bridge packages, register the Steam compatibility tool, and patch Steam:

```bash
just setup
# or: ./target/release/nucleon setup
```

### Configuring Games in Steam

> [!NOTE]
> Modern Steam on macOS does not provide a global **Steam Settings -> Compatibility** tab. Instead, compatibility tools are configured per game in each game's properties:

1. Restart Steam:
   ```bash
   pkill steam_osx && open -a /Applications/Steam.app
   ```
2. In your Steam Library, right-click any Windows game -> **Properties...** -> **Compatibility**.
3. Check **"Force the use of a specific Steam Play compatibility tool"**.
4. Select **"Nucleon (Game Porting Toolkit 4)"** from the dropdown menu.
5. Alternatively, launch games directly via CLI: `nucleon launch <AppID>`.

### Inspecting Status

Check the status of your installation, active runner, and Steam integration:

```bash
./target/release/nucleon status
```

### Launching Games

Launch any installed Steam game by its Application ID:

```bash
# Launch game normally (auto-detects DirectX version)
./target/release/nucleon launch <AppID>

# Launch with Apple Metal Performance HUD enabled
./target/release/nucleon launch <AppID> --hud

# Force a specific engine override (gptk or staging)
./target/release/nucleon launch <AppID> --engine staging
```

### Inspecting Executable Graphics APIs

To check which DirectX API and engine Nucleon will select for any `.exe` or game folder:

```bash
./target/release/nucleon detect /path/to/game.exe
```

### Managing VKD3D-Proton (Direct3D 12 for KosmicKrisp)

Binaries are never committed to source control. To enable Direct3D 12 translation over Vulkan 1.4 via Mesa KosmicKrisp:

#### 1. Download & Extract Official Release
Download the latest official release tarball (e.g. `vkd3d-proton-3.0.1.tar.zst` or `.tar.xz`) from [HansKristian-Work/vkd3d-proton Releases](https://github.com/HansKristian-Work/vkd3d-proton/releases):

```bash
# Example extracting via tar (supporting zstd or xz)
tar --zstd -xf vkd3d-proton-3.0.1.tar.zst -C /tmp/
# Extracted folder contains: /tmp/vkd3d-proton-3.0.1/x64/d3d12.dll
```

#### 2. Add Extracted Path to Nucleon
Attach the extracted directory to Nucleon using any of the following methods:

- **Method A: Via CLI Subcommand (Persistent)**
  ```bash
  nucleon vkd3d set-path /path/to/extracted/vkd3d-proton-3.0.1
  ```
- **Method B: During Initial Setup**
  ```bash
  nucleon setup --vkd3d-path /path/to/extracted/vkd3d-proton-3.0.1
  ```
- **Method C: Copy Directly to Nucleon Support Directory**
  ```bash
  mkdir -p "$HOME/Library/Application Support/nucleon/vkd3d-proton"
  cp -R /path/to/extracted/vkd3d-proton-3.0.1/x64 "$HOME/Library/Application Support/nucleon/vkd3d-proton/"
  ```
- **Method D: Environment Variable (Per-Session / Shell)**
  ```bash
  export VKD3D_PROTON_PATH=/path/to/extracted/vkd3d-proton-3.0.1
  ```

#### 3. Verify Detection
Check that Nucleon successfully detects the Direct3D 12 translation layer:

```bash
nucleon vkd3d status
# or check overall system status:
nucleon status
```

### Background Steam Update Guard & LaunchAgent

When Steam downloads background client updates, it can overwrite modified CEF WebUI chunks (`chunk~*.js`) or revoke ad-hoc codesigning on `steam_osx`. Nucleon includes an automated background guard service registered with macOS `launchd`:

```bash
# Check current guard status
./target/release/nucleon guard status

# Manually trigger verification and self-healing
./target/release/nucleon guard run

# Install / reload the LaunchAgent (watches Steam update paths with 0% CPU)
./target/release/nucleon guard install

# Uninstall the LaunchAgent
./target/release/nucleon guard uninstall

# Run continuous foreground watcher
./target/release/nucleon guard watch
```

- **FSEvents WatchPaths**: Monitored by `launchd` via `~/Library/LaunchAgents/com.nucleon.steam-guard.plist` without background battery/CPU drain.
- **In-Process Launch Hook**: `libnucleon.dylib` verifies and heals WebUI chunk patches asynchronously whenever Steam initializes.
- **Pre-Launch CLI Hook**: `nucleon launch <AppID>` automatically re-verifies ad-hoc signatures before invoking the game.

### Validating Window Presentation

Nucleon adheres to a strict zero-screen-capture policy. To verify that a game's window is actively drawing on screen, inspect WindowServer layer metadata directly:

```bash
./target/release/nucleon validate <AppID>
```

---

## Development & Quality Standards

Nucleon enforces strict linting, formatting, and test hygiene across the entire workspace:

```bash
# Run formatting check, Clippy (-D warnings), and test suite
just check
# or via cargo alias:
cargo check-all

# Format code across the workspace
just fmt
# or: cargo fmt --all

# Run Clippy with warnings treated as errors
just lint
# or: cargo lint

# Format code and auto-apply Clippy fixes
just fix
# or: cargo fix-all
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
