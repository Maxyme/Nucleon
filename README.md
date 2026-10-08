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

## Compatibility Tool Options & The Architecture Selector

In Steam, Nucleon registers compatibility tools with clear **`(runtime + graphics translation backend)`** labels so you always know exactly which Wine runtime and graphics translation pipeline each game is running on:

### Steam Compatibility Dropdown Options

| Option Label | Tool ID | Runner Runtime | Graphics Translation Backend | Best For |
| :--- | :--- | :--- | :--- | :--- |
| **`Nucleon (Wine + Automatic Graphics Backend)`** | `nucleon` | Active Wine | *Auto-detected* (KosmicKrisp / WineD3D) | **Recommended (Wine Default)**: Runs on Wine only. Automatically inspects binary imports and routes to the optimal Wine graphics backend (KosmicKrisp Vulkan for DX11/12/Vulkan vs WineD3D for legacy). |
| **`Nucleon (GPTK + Apple D3DMetal)`** | `nucleon-gptk` | Apple GPTK Wine | **Apple D3DMetal** (Metal 4, MSync, MetalFX) | **Separate Option**: Directly forces Apple Game Porting Toolkit 4 Wine with native D3DMetal translation for modern DirectX 11 & DirectX 12 games. |
| **`Nucleon (Wine + Mesa KosmicKrisp Vulkan)`** | `nucleon-kosmickrisp` | Active Wine | **Mesa KosmicKrisp Vulkan 1.4** (VKD3D-Proton / D7VK / DXVK) | Direct manual selection of Vulkan 1.4 driver on Metal 4 under Wine. Translates DX12 via VKD3D-Proton, DirectDraw/DX1–7 via D7VK, and native Vulkan. |
| **`Nucleon (Wine + WineD3D OpenGL)`** | `nucleon-wine` | Active Wine (Staging/CrossOver/etc.) | **WineD3D (macOS OpenGL 4.1)** | Direct manual selection of WineD3D for legacy DirectX 9, DirectX 10, and OpenGL titles. |

---

## Architecture: 64-Bit (x64) and 32-Bit (x86 / New WoW64)

Apple dropped support for 32-bit Mach-O binaries in macOS 10.15 (Catalina). Running Windows games—both modern 64-bit AAA titles and legacy 32-bit classics—requires two distinct execution strategies:

### 1. 64-Bit (`x86_64` / `win64`)
- Standard for modern DirectX 11 and DirectX 12 titles.
- Executed on Apple Silicon via Apple's **Rosetta 2** translation layer.
- Nucleon configures `ROSETTA_ADVERTISE_AVX=1` to ensure full AVX/AVX2 instruction set compatibility required by modern game engines (such as Frostbite, Unreal Engine 4/5, and Ego Engine).
- 64-bit Wine binaries (`lib/wine/x86_64-unix` and `lib/wine/x86_64-windows`) interface directly with host 64-bit macOS frameworks and libraries.

### 2. 32-Bit (`x86` / `win32` via New WoW64)
- Common for older Windows titles (DirectX 1 through 9, early DX10/11 games).
- Modern Wine (8.0+, 9.0+, 11.x) implements **New WoW64 mode** (Windows-on-Windows 64). In this architecture, 32-bit Windows code runs inside 64-bit host processes without needing any 32-bit host macOS libraries.
- System calls and graphics API invocations transition across the 32-bit to 64-bit boundary inside Wine's software thunk layer, calling host 64-bit APIs (`winemac.so`, Metal, and Vulkan).
- Translation layers like **D7VK** and **DXVK** supply 32-bit PE DLLs (`x86/ddraw.dll`, `x86/d3d9.dll`) that intercept 32-bit Direct3D calls in-process and translate them directly to host 64-bit Vulkan 1.4 on Metal 4, giving classic 32-bit games modern GPU performance on Apple Silicon.

---

## Complete Graphics API & DirectX Compatibility Matrix

Nucleon provides full coverage across every generation of DirectX, Vulkan, and OpenGL:

| Graphics API | Typical Binary Architecture | Primary Translation Engine | Translation Pipeline | Supported Options |
| :--- | :--- | :--- | :--- | :--- |
| **DirectX 12** | **x64** (rarely x86) | **Apple GPTK** or **KosmicKrisp** | • **Apple D3DMetal**: HLSL to Metal 4 MSL (Hardware DXR, MSync, MetalFX)<br>• **VKD3D-Proton**: Direct3D 12 to Vulkan 1.4 on Metal 4 | • `Nucleon (GPTK + Apple D3DMetal)`<br>• `Nucleon (Wine + Mesa KosmicKrisp Vulkan)`<br>• `Nucleon (Wine + Automatic Graphics Backend)` |
| **DirectX 11** | **x64** / **x86** | **Apple GPTK** or **KosmicKrisp** | • **Apple D3DMetal**: Direct3D 11 to Metal 4<br>• **DXVK**: Direct3D 11 to Vulkan 1.4 on Metal 4 | • `Nucleon (GPTK + Apple D3DMetal)`<br>• `Nucleon (Wine + Mesa KosmicKrisp Vulkan)`<br>• `Nucleon (Wine + Automatic Graphics Backend)` |
| **DirectX 10 / 10.1** | **x64** / **x86** | **WineD3D** or **KosmicKrisp** | • **WineD3D**: Built-in Direct3D 10 to OpenGL 4.1<br>• **DXVK**: Direct3D 10 to Vulkan 1.4 on Metal 4 | • `Nucleon (Wine + WineD3D OpenGL)`<br>• `Nucleon (Wine + Mesa KosmicKrisp Vulkan)`<br>• `Nucleon (Wine + Automatic Graphics Backend)` |
| **DirectX 9 / 8** | **x86** (rarely x64) | **WineD3D** or **KosmicKrisp** | • **WineD3D**: Built-in Direct3D 9 to OpenGL 4.1<br>• **DXVK**: Direct3D 9 to Vulkan 1.4 on Metal 4 | • `Nucleon (Wine + WineD3D OpenGL)`<br>• `Nucleon (Wine + Mesa KosmicKrisp Vulkan)`<br>• `Nucleon (Wine + Automatic Graphics Backend)` |
| **DirectDraw / DX1–7** | **x86** | **KosmicKrisp (D7VK)** or **WineD3D** | • **D7VK**: DirectDraw & Direct3D 1–7 to Vulkan 1.4 on Metal 4<br>• **WineD3D**: Legacy built-in `ddraw.dll` over OpenGL 4.1 | • `Nucleon (Wine + Mesa KosmicKrisp Vulkan)`<br>• `Nucleon (Wine + WineD3D OpenGL)`<br>• `Nucleon (Wine + Automatic Graphics Backend)` |
| **Vulkan (Native)** | **x64** / **x86** | **Mesa KosmicKrisp** | • **Mesa KosmicKrisp**: Khronos-conformant Vulkan 1.4 ICD driver directly on Metal 4 | • `Nucleon (Wine + Mesa KosmicKrisp Vulkan)`<br>• `Nucleon (Wine + Automatic Graphics Backend)` |
| **OpenGL** | **x86** / **x64** | **WineD3D / Host** | • **Native macOS OpenGL 4.1** or **Mesa Zink** over KosmicKrisp | • `Nucleon (Wine + WineD3D OpenGL)`<br>• `Nucleon (Wine + Automatic Graphics Backend)` |

---

### Why Modern Games Need D3DMetal or KosmicKrisp (Not Vanilla WineD3D)

Running modern Windows games (DirectX 11 & 12) under vanilla Wine with built-in `WineD3D` on macOS often fails with errors such as:
```text
Failed to create D3D device
```

**Technical Explanation:**
- Wine's built-in `WineD3D` translates Direct3D calls into desktop OpenGL.
- Apple deprecated OpenGL in macOS 10.14 (Mojave) and froze its driver implementation at **OpenGL 4.1** (Core Profile) in 2018.
- Modern DirectX 11 (Feature Level 11_0+) and DirectX 12 games require capabilities introduced in OpenGL 4.3+ and modern extensions—such as compute shaders (`GL_ARB_compute_shader`), Shader Storage Buffer Objects (SSBOs), multi-draw indirect, tessellation, and advanced floating-point texture formats—which macOS OpenGL 4.1 completely lacks.
- To run modern games, Nucleon bypasses deprecated macOS OpenGL entirely:
  - **Apple D3DMetal (under GPTK)**: Translates HLSL Direct3D bytecodes directly into Metal Shading Language (MSL) and executes on Apple Silicon GPUs with full hardware acceleration.
  - **Mesa KosmicKrisp (under Wine)**: Implements a full Khronos-conformant **Vulkan 1.4** driver on top of Metal 4 using Mesa's NIR compiler, enabling DXVK and VKD3D-Proton pipelines directly in Wine.
  - **WineD3D**: Retained specifically for DirectX 9, DirectX 10, and OpenGL titles that fit within OpenGL 4.1 specifications.

---

### Automatic Graphics Routing for Wine (Auto Runner)

When using **`Nucleon (Wine + Automatic Graphics Backend)`**, Nucleon runs under the **Wine runtime only** and automatically selects the optimal graphics translation backend for Wine:

| Graphics API | Imported DLLs | Dispatched Engine (Wine Only) | Pipeline Characteristics |
| :--- | :--- | :--- | :--- |
| **DirectX 12** | `d3d12.dll`, `dxgi.dll` | **Wine + Mesa KosmicKrisp Vulkan** | VKD3D-Proton Direct3D 12 -> Vulkan 1.4 on Metal 4 |
| **DirectX 11** | `d3d11.dll`, `dxgi.dll` | **Wine + Mesa KosmicKrisp Vulkan** | DXVK Direct3D 11 -> Vulkan 1.4 on Metal 4 |
| **Vulkan 1.3 / 1.4** | `vulkan-1.dll` | **Wine + Mesa KosmicKrisp Vulkan** | Khronos-conformant Vulkan 1.4 driver on Metal 4 (Mesa NIR compiler) |
| **DirectDraw / DX1–7** | `ddraw.dll` | **Wine + Mesa KosmicKrisp Vulkan** | D7VK translation directly to Vulkan 1.4 (bypasses OpenGL) |
| **DirectX 10** | `d3d10.dll`, `d3d10_1.dll`, `d3d10core.dll` | **Wine + WineD3D OpenGL** | Full `wined3d` pipeline |
| **DirectX 9 & Older** | `d3d9.dll`, `d3d8.dll` | **Wine + WineD3D OpenGL** | Upstream Wine legacy pipeline |
| **OpenGL** | `opengl32.dll` | **Wine + WineD3D OpenGL** | Native macOS OpenGL |

> [!NOTE]
> The **Auto runner is for Wine only**. It never switches to the Apple GPTK runner. If you wish to use Apple's proprietary `D3DMetal` pipeline on Apple GPTK Wine, select the separate **`Nucleon (GPTK + Apple D3DMetal)`** option in Steam.

#### How Dynamic Dispatch Works
1. **PE Import Table Analysis**: When launching a title, `nucleon-runner` parses the Windows Portable Executable (PE) headers and Import Address Table (IAT) using the Rust `object` engine.
2. **Wine Auto Dispatch**:
   - Modern DX11/12, Vulkan, and D7VK titles route to Mesa KosmicKrisp (`VK_DRIVER_FILES=.../libkosmickrisp_icd.json`).
   - Legacy DX9/10 titles route to WineD3D.
   - If KosmicKrisp is not installed, Nucleon falls back to WineD3D with an informative log message.
3. **Manual Overrides**: You can override engine selection in Steam's Compatibility dropdown or via CLI using `--engine <auto|gptk|kosmickrisp|staging>` or the `NUCLEON_ENGINE` environment variable.

---

## Runtime & Engine Compatibility

- **Apple Game Porting Toolkit (GPTK 4 / GPTK 2)**: Fully supported and recommended for DirectX 11 and DirectX 12 titles via `D3DMetal.framework`.
- **Mesa KosmicKrisp**: Full Vulkan 1.4 conformant driver implemented on Metal 4 for Apple Silicon (macOS 26+). Recommended for native Vulkan titles and open-source Direct3D via DXVK, VKD3D-Proton, and D7VK.
- **VKD3D-Proton (Direct3D 12 -> Vulkan 1.4)**: Translates Direct3D 12 calls to Vulkan 1.4 when running with KosmicKrisp. Binaries are never tracked in git; you can extract official releases and configure Nucleon via `nucleon vkd3d set-path <dir>`, `nucleon setup --vkd3d-path <dir>`, or `VKD3D_PROTON_PATH`.
- **D7VK (DirectDraw / Direct3D 1–7 -> Vulkan 1.4)**: Translates legacy DirectDraw and Direct3D 1 to 7 calls to Vulkan 1.4 when running with KosmicKrisp, bypassing deprecated macOS OpenGL. Binaries are never tracked in git; configure via `nucleon d7vk fetch`, `nucleon d7vk set-path <dir>`, `nucleon setup --d7vk-path <dir>`, or `D7VK_PATH`.
- **Wine Runtimes (Wine Selector)**: Seamless integration with Wine-Staging (`brew install --cask wine-staging`), Heroic Games Launcher Wine, Whisky Wine, CrossOver, and custom builds. Manage and switch active runtimes via `nucleon wine`.

---

## Getting Started

### Prerequisites

- macOS 14 (Sonoma) or macOS 15 (Sequoia) on Apple Silicon.
- [Rust](https://rustup.rs/) (1.80+ recommended).
- [just](https://github.com/casey/just) command runner (`brew install just`).
- Xcode Command Line Tools (`xcode-select --install`).
- Apple Game Porting Toolkit components (`D3DMetal.framework`, `libd3dshared.dylib`), or Wine-Staging.
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

### Setting Up Apple Game Porting Toolkit (GPTK)

Nucleon uses Apple's native `D3DMetal.framework` and `libd3dshared.dylib` from the **Apple Game Porting Toolkit (GPTK 4 / GPTK 2)** for hardware-accelerated DirectX 11 and DirectX 12 translation on Apple Silicon.

Apple provides these components inside a disk image (`.dmg`) available from [Apple Developer: Game Porting Toolkit](https://developer.apple.com/games/game-porting-toolkit/).

#### 1. Download & Mount the Apple GPTK DMG
Download **Game Porting Toolkit** (`Game_Porting_Toolkit_4.0_beta_2.dmg`) from [developer.apple.com/games/game-porting-toolkit/](https://developer.apple.com/games/game-porting-toolkit/) and mount it:

```bash
hdiutil attach ~/Downloads/Game_Porting_Toolkit_4.0_beta_2.dmg
# Mounts to: /Volumes/Game Porting Toolkit 4.0 beta 2 (or /Volumes/Game Porting Toolkit)
```

#### 2. Export GPTK Components to a Directory
Export the components from the mounted disk image to a local directory:

```bash
# Create a local directory for GPTK components:
mkdir -p "$HOME/Developer/gptk"

# Copy D3DMetal.framework and libd3dshared.dylib:
cp -R "/Volumes/Game Porting Toolkit 4.0 beta 2/redist/lib/external/D3DMetal.framework" "$HOME/Developer/gptk/"
cp "/Volumes/Game Porting Toolkit 4.0 beta 2/redist/lib/external/libd3dshared.dylib" "$HOME/Developer/gptk/"

# Unmount the DMG when finished:
hdiutil detach "/Volumes/Game Porting Toolkit 4.0 beta 2"
```

#### 3. Register the GPTK Directory with Nucleon
Attach the exported directory to Nucleon using any of the following methods:

- **Method A: Via CLI Subcommand (Persistent)**
  ```bash
  nucleon gptk set-path "$HOME/Developer/gptk"
  ```
- **Method B: During Initial Setup**
  ```bash
  nucleon setup --gptk-path "$HOME/Developer/gptk"
  ```
- **Method C: Environment Variable (Per-Session / Shell)**
  ```bash
  export NUCLEON_GPTK_PATH="$HOME/Developer/gptk"
  ```

#### 4. Verify Detection
Check that Nucleon successfully detects `D3DMetal.framework` and `libd3dshared.dylib`:

```bash
nucleon gptk status
# or check overall system status:
nucleon status
```

### Managing Wine Runtimes (including CrossOver)

Nucleon automatically discovers existing Wine installations (Heroic Games Launcher, Homebrew, Whisky, and CrossOver), and treats CrossOver as a first-class standard Wine runtime under the unified Wine selector:

```bash
# List all discovered and custom Wine runtimes:
nucleon wine list

# Switch active Wine runtime directly to CrossOver:
nucleon wine use crossover

# Or point directly to any Wine or CrossOver path:
nucleon wine use /Applications/CrossOver.app

# Register a custom Wine/CrossOver installation under a custom name:
nucleon wine add crossover-24 /Applications/CrossOver.app

# Run end-to-end setup pointing to a custom Wine or CrossOver:
nucleon setup --wine-path /Applications/CrossOver.app

# Per-game Steam Launch Option override:
NUCLEON_WINE=/Applications/CrossOver.app %command%
# or by identifier:
NUCLEON_WINE=crossover %command%
```

Nucleon automatically inspects and resolves nested CrossOver bundle paths (such as `CrossOver.app`, `Contents/SharedSupport/CrossOver`, `bin/wine`, and `bin/wine64`), detects its version, and configures the environment with `CX_ROOT`, `DYLD_FALLBACK_LIBRARY_PATH`, and DLL search paths.

### Safe Process Supervision (`sysinfo` & Tree Killing)

`nucleon-runner` features zero unsafe code (`#![forbid(unsafe_code)]`) and does not rely on raw `libc::kill` calls:
- **Kernel-level Process Table Tracking**: Uses `sysinfo` to monitor running processes directly via OS APIs instead of polling `/bin/ps` every 500ms.
- **Recursive Process Tree Termination (`kill_tree`)**: When a termination signal (`SIGTERM` or Steam "Stop") is received, Nucleon traverses the process hierarchy to recursively terminate the child process and all its descendants.
- **Prefix Isolation**: Identifies and terminates orphaned background Wine processes associated with the prefix while leaving unrelated system processes untouched.

### Staging Valve Client Bridge Libraries

Nucleon requires Valve's Windows client bridge libraries (`steamclient64.dll`, `tier0_s64.dll`, etc.) to bridge communication between Windows games in Wine and the native macOS Steam client.

Rather than bloating the codebase with in-process decompression crates, C build tools, or complex download scripts, Nucleon uses a declarative asset manifest ([`assets/bridge-manifest.json`](assets/bridge-manifest.json)) that maps expected DLLs to their final destinations.

#### Step-by-Step Staging

1. **Download official client packages**:
   ```bash
   curl -fSL -O https://client-update.akamai.steamstatic.com/bins_misc_ubuntu12.zip.3f92810725ee673827371a0470cd4f8c7ea8cfae
   curl -fSL -O https://client-update.akamai.steamstatic.com/bins_win64.zip.36f5d9202e79ab2aa3e3c5902e84bbd799d31fc0
   ```

2. **Unpack into a temporary directory**:
   ```bash
   ditto -x -k bins_misc_ubuntu12.zip.* /tmp/valve-bridge/
   ditto -x -k bins_win64.zip.* /tmp/valve-bridge/
   ```

3. **Stage into Nucleon**:
   Pass the path to Nucleon, which scans the extracted files against `bridge-manifest.json` and copies all required and optional libraries into `~/Library/Application Support/nucleon/bridge/`:
   ```bash
   # Via just:
   just stage-bridge /tmp/valve-bridge

   # Or via CLI flag:
   ./target/release/nucleon setup --bridge-path /tmp/valve-bridge

   # Or via environment variable:
   export NUCLEON_BRIDGE_PATH=/tmp/valve-bridge
   just setup
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
4. Select your preferred compatibility tool from the dropdown menu:
   - **`Nucleon (Wine + Automatic Graphics Backend)`** *(Recommended / Default)*: Automatically detects the game's graphics API and launches with the optimal runtime and graphics backend on Wine.
   - **`Nucleon (GPTK + Apple D3DMetal)`**: Separate option forcing Apple Game Porting Toolkit 4 Wine with native D3DMetal for DirectX 11/12.
   - **`Nucleon (Wine + Mesa KosmicKrisp Vulkan)`**: Forces Wine with Mesa KosmicKrisp Vulkan 1.4 (for Vulkan, VKD3D-Proton, or D7VK).
   - **`Nucleon (Wine + WineD3D OpenGL)`**: Forces active Wine with built-in WineD3D (for legacy DirectX 9/10 and OpenGL).
5. Alternatively, launch games directly via CLI: `nucleon launch <AppID>`.

### Removing Nucleon from Steam UI

To unregister Nucleon and completely remove all compatibility tools and UI patches from Steam:

```bash
just unregister
# or: ./target/release/nucleon steam unregister
```

This performs a comprehensive reset:
- **Compatibility Tool Bundles**: Removes all `nucleon*` folders from `~/Library/Application Support/Steam/compatibilitytools.d/`.
- **Per-Game Mappings**: Cleans any titles mapped to Nucleon in `~/Library/Application Support/Steam/config/config.vdf`.
- **WebUI Chunk Patches**: Reverts JavaScript chunk modifications in `steamui/` and flushes the CEF cache.
- **Steam.app Bundle**: Restores the stock `Info.plist`, deletes `nucleon.dylib`, and re-signs Steam.
- **Background Guard**: Unloads and removes the `com.nucleon.steam-guard` LaunchAgent daemon.

After running unregister, restart Steam (`pkill steam_osx && open -a /Applications/Steam.app`) to refresh the interface.


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

### Managing D7VK (DirectDraw / Direct3D 1–7 for KosmicKrisp)

DirectDraw and early Direct3D (DirectX 1 to 7) titles run via D7VK over Vulkan 1.4 on Mesa KosmicKrisp, avoiding macOS's deprecated OpenGL stack:

#### 1. Fetch Automatically or Download Release
- **Option A: Automatic fetch via Nucleon CLI**
  ```bash
  nucleon d7vk fetch
  ```
- **Option B: Manual download from GitHub**
  Download `d7vk-v2.3.zip` from [WinterSnowfall/d7vk Releases](https://github.com/WinterSnowfall/d7vk/releases) and unzip it.

#### 2. Configure Path
- **Via CLI Subcommand:**
  ```bash
  nucleon d7vk set-path /path/to/extracted/d7vk-v2.3
  ```
- **During Initial Setup:**
  ```bash
  nucleon setup --fetch-d7vk
  # or point to existing path:
  nucleon setup --d7vk-path /path/to/extracted/d7vk-v2.3
  ```
- **Environment Variable:**
  ```bash
  export D7VK_PATH=/path/to/extracted/d7vk-v2.3
  ```

#### 3. Verify Detection
```bash
nucleon d7vk status
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
