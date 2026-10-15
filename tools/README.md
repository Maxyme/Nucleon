# ARM64 Wine/FEX experiment

This builds a separate experimental runtime on Apple Silicon macOS. Python 3.12+,
Xcode, CMake, Ninja, current Bison and Flex are required. It does not register a
Steam compatibility tool, select a runtime in Nucleon, or launch Steam.

Sources and the LLVM-MinGW compiler are pinned in
[`arm64-fex.toml`](../assets/wine/recipes/arm64-fex.toml). The compiler archive is
verified before extraction. Builds use `target/arm64-fex`; existing game prefixes
and installed runtimes are not build inputs or destinations.

## MacRunner / HyperBridge

```sh
python3 tools/arm64_fex.py fetch
python3 tools/arm64_fex.py doctor
python3 tools/arm64_fex.py build-fex
python3 tools/arm64_fex.py build-wine
python3 tools/arm64_fex.py probe
```

The Wine base is MacRunner's public fork of CrossOver 26.1.0/Wine 11.0 source.
It does not require the installed CrossOver application. Wine and its retained
HyperBridge dependency keep their LGPL/MIT licenses and upstream notices.
The FEX build applies HyperBridge's ordered Darwin patches to the pinned FEX
base. Only the x64 emulator is built on this route; it is not yet a complete
32/64-bit game runtime.

Local validation on 2026-10-15:

- Wine and ARM64EC FEX compiled and installed into the work directory.
- LLVM identifies the emulator as ARM64EC; its Mach-O companions are ARM64.
  The raw PE machine field alone is insufficient to identify an ARM64EC image.
- Building Wine with Vulkan disabled needed the included, small `vulkan.c`
  patch: the fork otherwise referenced an undefined Vulkan library name.
- `wine --version` works, but launching the owned Windows workload exits with
  `macrunner-signed32: reserve PAGEZERO: Cannot allocate memory`.
- A native host probe confirms 16 KiB host pages and denies the low mapping.
  The selected Wine loader needs appropriate cross-architecture provisioning.

Compilation is not recorded as successful Windows execution. This source-built
runtime is not ready for game use.

## Entitlement-free alternative

```sh
python3 tools/arm64_fex.py fetch --variant tolkara
python3 tools/arm64_fex.py build-fex --variant tolkara
python3 tools/arm64_fex.py build-wine --variant tolkara
python3 tools/arm64_fex.py probe --variant tolkara --window
```

This uses the separately pinned Tolkara Wine/FEX branches and stores products
under `target/arm64-fex/tolkara`. Its two emulator modules use the matching Wine
fork's relocated guest-address and thread-local-state conventions. They are
not replacements to copy into the MacRunner-based runtime. This route does not
build a native FEX Unix companion; unsupported optional UnixLib facilities use
the fork's fallback behavior.

On the test Mac this variant passes memory operations, thread atomics,
exception handling, self-modifying x64 code, child processes and a bounded GDI
window test. It uses software x86 memory ordering and requires neither Rosetta
nor Apple cross-architecture provisioning for these tests. The x86 emulator was
used during prefix initialization; broader 32-bit gameplay is not established.

Two downstream Wine patches supply the missing ARM64EC `___chkstk_ms` export
using Wine's existing stack-probe implementation, and emulate bounded scalar
reads of the relocated Windows shared-data page. The latter copies the original
read decoder from the pinned MacRunner Wine source, checks its checksum, and
reads Wine's live shared mapping. Stores, out-of-page accesses, unsupported
instructions and alignment failures retain the ordinary exception path. It is
enabled by `NUCLEON_EMULATE_KUSD=1` in the prototype environment. The CPU test
checks both valid reads and preserved write/bounds protection. Its negative
control disables the handler and must fail at the shared-data read.

Read `probes/result.json` inside the selected work directory for actual
execution status. Build completion alone does not establish successful execution.

## Offscreen Direct3D 11 / Metal

```sh
python3 tools/arm64_fex.py fetch --variant tolkara --reference
python3 tools/arm64_fex.py probe-dxmt --variant tolkara
```

This optional test uses graphics components from the checksum-pinned MacRunner
archive, rather than rebuilding DXMT. It verifies the archived component hashes,
stages private copies and ad-hoc signs the unprivileged ARM64 Metal companion.
The original reference bundle and privileged loader are not modified. The DXMT
PE frontend is x64 code translated by FEX; its Unix companion is ARM64. This is
not yet an ARM64EC-native DXMT frontend build.

The test initializes a dedicated prefix before staging the matching native DLLs,
creates a feature-level 11_0 device, checks a GPU texture copy and compiles vertex
and pixel shaders. It draws an offscreen triangle, validates green center/black
corner pixels and saves `probes/triangle.ppm`. A second run disables drawing and
must fail the center-pixel check. Results and component hashes are recorded in
`probes/d3d11-result.json` under `target/arm64-fex/tolkara`.

The offscreen tests alone do not validate swap-chain presentation, game workloads,
performance, media playback or Steamworks. Wine needs local IPC sockets and
window/GPU access, which the development sandbox may restrict; an IPC permission
error is distinct from a Wine/FEX compatibility failure.

## DiRT Rally 2.0 launch

On 2026-10-15, DiRT Rally 2.0 reached its title screen/menu with this Tolkara
runtime, FEX and the reference DXMT components; the user confirmed the visible
result. The game connected to the signed-in native macOS Steam client through
Nucleon's locally installed ARM64 Unix `lsteamclient` companion and translated
x64 PE bridge. No Windows Steam installation was needed. Racing, performance,
controllers and complete game compatibility have not been validated.

The downstream `wine-tolkara-dxmt-window.patch` provides the reference DXMT's
legacy window-function table and adapts its window-data layout. Before an
OpenGL/Vulkan client surface exists, it attaches Metal to the real toplevel
window's content view. The launch log confirms `attached_to_hwnd=1`; no orphan
window fallback is enabled. This private adapter targets the pinned DXMT ABI,
not arbitrary third-party window integrations.

After the preceding build and DXMT staging steps, start native Steam and run:

```sh
python3 tools/launch_dirt_fex.py
```

The helper prefers the registered managed runtime and calls the normal Rust
runner. If none is installed, it can assemble an existing developer build. Build it first with
`cargo build -p nucleon-runner`. It requires Nucleon's local ARM64 bridge,
including `aarch64-unix/lsteamclient.so`. Its isolated compatdata root is
`target/arm64-fex/integrated-compatdata/690790`; the runner adds
`nucleon/arm64-fex/<family>/pfx`. `--runtime` uses an existing complete descriptor
runtime; `--game-dir`, `--bridge`, `--sdk` and `--work` override local locations.
Rust owns prefix initialization, private bridge binding/signing, graphics staging
and process cleanup. The private diagnostic profile leaves the production profile
and one-shot overrides intact. Runner output, a separate Wine log and a JSON
receipt go into `dx12/logs/`. `--timeout SECONDS` starts counting after the game
launcher is spawned; preparation is excluded. `--debug` enables verbose traces. Steam cloud/user saves still use the real Steam
user-data directory. Close an existing prototype game before relaunching.

The bridge is a local prerequisite, not a new pinned build product; this helper
does not claim to reproduce its build from the prototype recipe.

## Owned workload and evidence

The probe compiles `runtime-probes/windows.c` as an x64 executable and tests
memory operations, four-thread atomic increments, Windows exception handling,
self-modifying x64 code, and a child process with a known exit code. `--window`
also creates, paints and closes an owned test window after one second. It uses a
prefix under the experiment directory, a bounded run and prefix-scoped server
cleanup. It never starts Steam or a game. The native host probe runs separately;
its permissions describe that probe, not a separately provisioned Wine loader.

Logs are in `logs/`, compiler/runtime receipts are `build.json`, and probe
results are in `probes/`. MacRunner and Tolkara products have distinct log and
prefix directories. Preserve failed logs when investigating. Wine/FEX binaries,
their matching source trees and archives stay under the ignored build directory.

For the separately downloaded MacRunner 1.0.9 reference archive, the published
SHA256 is in the recipe. `check-reference` verifies that archive and the extracted
launcher signature; it does not execute the launcher. On this Mac, the archive
checksum matched but `codesign --verify --strict` rejected the Wine loader and
`codesign -d --entitlements` reported an invalid entitlements blob. Its extracted
loader bytes matched those in the archive. It was not used to run Windows code.

```sh
python3 tools/arm64_fex.py check-reference
```

This optional check expects the archive in the work directory and its extracted
bundle under `reference/`. A full dependency rebuild, graphics integration,
portable signing and Nucleon's native Steam bridge are later milestones.

## Experimental Direct3D 12 / DMC5

The DX12 work has a separate recipe, source checkout, Wine build, driver, caches
and prefixes under `target/arm64-fex/dx12`. It reuses the pinned Tolkara FEX
modules from the CPU prototype. It does not select an installed Nucleon runtime.

Additional build dependencies are the Vulkan macOS SDK (tested: 1.4.363.0),
Homebrew LLVM and matching SPIR-V LLVM translator, glslang, SPIR-V tools and Ninja.
The driver builder creates its own Python environment with pinned packages.
Build the base Tolkara/FEX experiment first, then:

```sh
python3 tools/arm64_fex_dx12.py fetch
python3 tools/arm64_fex_dx12.py build-driver
python3 tools/arm64_fex_dx12.py build-wine
python3 tools/arm64_fex_dx12.py probe-driver
python3 tools/arm64_fex_dx12.py probe-vulkan
python3 tools/arm64_fex_dx12.py probe-d3d12
```

`--sdk`, `--work` and `--jobs` override local paths and parallelism. `fetch` checks
pinned sources and graphics archive hashes. The Wine build enables the Khronos
Vulkan loader; the driver build applies Steamac's pinned KosmicKrisp patch set to
its exact Mesa base. It preserves changed source trees instead of resetting them.
The private driver directory precedes the SDK in `DYLD_LIBRARY_PATH`: otherwise
macOS can substitute the SDK's same-named, unpatched driver even when the ICD
manifest names the private path.

Actual checks on 2026-10-15:

- Patched ARM64 KosmicKrisp: capability, geometry, robustness, GPU device-address,
  single-texel alignment and query GPU checks passed. The sparse suite reports
  **two D32 sparse depth rendering failures**, in one and two render passes.
  `probe-driver` deliberately exits nonzero and records them. It is not a fully
  passing driver; geometry-strip transform-feedback limitations also remain
  documented upstream. Ordinary sparse buffer/image checks passed.
- x64 Windows Vulkan through FEX: native device creation, required feature-chain
  queries and a Win32 surface passed. The log identifies the patched Mesa driver.
- DX12: compute dispatch and exact readback, command fences, triangle readback,
  60 window presents and resizing passed. Disabling drawing produced the expected
  black center pixel and failure, verifying the positive readback check.

The DX12 shader probe needs a licensed x64 Microsoft `d3dcompiler_47.dll` because
this Wine's builtin compiler does not implement its SM5 compute shader. By default
it stages a private copy from the installed DiRT Rally 2.0 directory; set
`NUCLEON_PROBE_D3DCOMPILER=/absolute/path/d3dcompiler_47.dll` to use another licensed
copy. The receipt records its hash. This DLL is neither downloaded nor included
in Nucleon's source or distributions.

For the real native Steam connection, start Steam and sign in before:

```sh
python3 tools/arm64_fex_dx12.py probe-steam
python3 tools/launch_dmc5_fex.py
```

The launcher requires Nucleon's existing `bridge/aarch64-unix/lsteamclient.so`,
`bridge/x86_64-windows/lsteamclient.dll` and the original
`bridge/steamclient64.dll.valve`, plus the existing `bridge/steam.exe` helper. `--bridge` and `--game-dir` override local paths;
`NUCLEON_STEAM_BRIDGE` overrides the bridge location for `probe-steam`. The loader
probe validates a real Steam pipe/user and the installed game SDK's
`SteamAPI_IsSteamRunning`/`SteamAPI_Init`, verifies the exported function still
belongs to the original Valve image and checks its unchanged on-disk hash. Its
negative control disables the redirect and must fail to load that Windows client
without its Windows-only dependencies. A second negative control sets the
Windows Steam PID to zero and requires the SDK to report Steam unavailable.
`NUCLEON_STEAM_API` can select another installed, licensed game SDK DLL for this
probe; the default is DMC5's `steam_api64.dll`. It is not redistributed.

The Wine redirect is opt-in, preserves Valve's DLL on disk and forwards only
AMD64 images without ARM64EC metadata to the x64 bridge. Its load flags are
adapted to this older Wine fork. The bridge's Unix half stays native ARM64 and
is bound to the selected runtime's own ntdll, using signed private copies.

`launch_dmc5_fex.py` uses the same normal Rust runner and descriptor-backed
runtime as the DiRT helper (`cargo build -p nucleon-runner` first). DMC5 requires
the redirect-capable DX12 Wine build, even for the DX11 diagnostic control.
It launches through the existing Steam helper using a Windows game path and
forces DX12 in a backed-up INI,
with API fallback disabled. It restores the original bytes on normal exit,
errors, interruption or SIGTERM. The per-run INI backup and JSON receipt remain
under `logs/` for recovery after an uncatchable termination. Close the game before
stopping the launcher; settings changed in-game during this experiment are
replaced by the original INI on exit. Steam user saves remain real. Concurrent
configuration changes are rejected with a lock keyed by the actual INI path.
The runner also locks its isolated prefix through cleanup. `--timeout SECONDS` makes a bounded
diagnostic run measured after launcher spawn, excluding prefix preparation.
Both APIs reuse compatible native state under
`target/arm64-fex/integrated-compatdata/601150/nucleon/arm64-fex/<family>/pfx`;
`--api dx11` selects DXMT and stages the corresponding graphics set. `--debug`
adds timestamped dialog/exception/process logging and opt-in Wine process-exit
and quit-message stack traces. Receipts record duration and leave gameplay explicitly unconfirmed
regardless of the process exit code.

The user confirmed DMC5's title screen/menu on 2026-10-15 through the ARM64/FEX
DX12 path, with VKD3D-Proton 3.0.1 and a 1920×1080, three-buffer swapchain.
The initial direct launch created its swapchain but showed a black window and
exited zero: the SDK's Windows Steam-presence check failed. The existing helper
supplies its live Windows PID while the bridge continues to use real native
Steam. The regression probe checks this distinction using the actual game SDK.

The user confirmed that the menu run closed unexpectedly, with a Wine heap
fault during teardown (child status
`c0000005`, helper exit 5). The game posted its quit message before that fault.
A repeat DX12 run stayed open for its full 125-second limit without a recorded
quit or unhandled exception; the timeout cleanup restored the original INI.
The DX11 control also took the same scripted quit path, exiting zero at about
100 seconds. The earlier exit remains unresolved. This is menu confirmation,
not a claim of stable gameplay or video playback.
The native driver's two sparse-depth failures also remain. Production Nucleon
integration stays gated on those broader checks. `dmc5-validation.json` records
visual confirmation separately from process exit and smoke-test results.

## Experimental Steam integration

The game launchers no longer own Wine setup or cleanup. To expose their runtime
through Steam's normal compatibility UI without launching Steam:

```sh
cargo build -p nucleon-cli -p nucleon-runner
python3 tools/arm64_fex_dx12.py prepare-runtime
./target/debug/nucleon wine fex install /absolute/path/printed/by/prepare-runtime --register
```

See [runtime descriptors and profiles](../RUNTIME-DESCRIPTORS.md) for the native
option, opt-in game selection, separate prefix namespace and compatibility rules.
The managed copy lives under Nucleon's Application Support directory and survives
cleaning `target/`. Its native dependency audit rejects external library paths;
normal Steam launches need no compiler or Vulkan SDK. Old generations are retained,
and prefixes stage only changed graphics/bridge outputs. Use
`nucleon wine fex verify` for a full integrity check. These checks do not validate
gameplay. Diagnostic helpers record executable metadata by default; `--debug`
also records its SHA256. See the runtime contract for archive imports and the
remaining distribution-notice requirements.

Diagnostic wiring tests use a fake runner and do not execute Wine or games:

```sh
python3 -m unittest discover -s tools -p test_nucleon_fex.py
```
