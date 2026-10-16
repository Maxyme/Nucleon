# Native ARM64 Wine + FEX on macOS

Investigation and prototype date: 2026-10-15. This is an isolated experimental
runtime. DiRT Rally 2.0 and DMC5 reached their title screen/menu through native
Steam, confirmed by the user. Gameplay and performance remain unverified;
DMC5 had an unexpected later exit. The separate DX12 runtime passes its GPU
probes, with the driver limitations recorded below.

Runtime capability descriptions and architecture-aware launch/prefix preparation
are now implemented in Nucleon core. See [the runtime contract](RUNTIME-DESCRIPTORS.md)
for descriptor layout, checks and remaining integration work. The research notes
below include the earlier feasibility checklist and should be read with the
implementation results at the end of this document.

The executable prototype and its pinned recipe are documented in
[tools/README.md](tools/README.md). Both Wine/FEX variants were built from
source. MacRunner-based Wine stops at its loader's low-address reservation; its
reference loader also failed signature verification on this machine despite a
matching archive checksum.

The Tolkara-based variant, with downstream Wine compatibility patches and the
pinned MacRunner shared-data decoder, passes x64 CPU, exceptions, threads,
self-modifying code, protected shared-data reads, child processes and a GDI
window test without Rosetta or restricted provisioning. Reference DXMT graphics
components also passed feature-level 11_0 device creation, GPU copy/readback and
offscreen shader/triangle rendering. The DXMT PE frontend remains translated x64
code, with a native ARM64 Unix companion; it was not rebuilt in this experiment.
The later DiRT experiment also established presentation and a native Steam
connection. DMC5 compatibility and performance remain unverified. The separate
[DX12 investigation](docs/dmc5-arm64-fex-dx12-investigation.md) records measured
Vulkan-driver blockers and the recommended work.

## Recommended first experiment

The first DiRT menu launch is complete using translated x64 DXMT and an ARM64
Unix companion. Native Steam integration already works for that game; Windows
Steam was unnecessary. Next validate DMC5's separate Steam loader requirements
and DX11/FEX behavior, then pursue DX12 using the dedicated investigation below.
ARM64EC graphics frontends remain a later optimization.

The components have separate jobs:

| Component | Job |
| --- | --- |
| FEX | Translate the game's x86/x64 CPU instructions to ARM64 |
| ARM64/ARM64EC Wine | Implement Windows APIs and the native/emulated calling boundary |
| ARM64EC DXMT + ARM64 native companion | Translate Direct3D 11 to Metal |
| Steam integration | Supply the game's Steam session, callbacks and client interfaces |

There is no Linux VM or x86 Linux root filesystem in this design. The game
remains translated x64 code; making Wine native does not make the game native.
[FEX's ARM64EC documentation](https://wiki.fex-emu.com/index.php/Development:ARM64EC)
explains why host libraries can be ARM64 while the Windows application is x64.

## Available starting points

**A standalone packaged option exists: MacRunner.** Its
[application repository](https://github.com/t0b1kent/macrunner-app) describes
the October 7 development preview, 1.0.9, as a complete signed bundle containing
Wine, its FEX-based HyperBridge engine and DXMT. This is an alternative to
installing CrossOver. These are maintainer-reported results; this investigation
has not installed the package, audited every bundled component's provenance or
verified DiRT/DMC5. Public FEX sources do not by themselves establish a complete
independent rebuild of the whole bundle.

**Tolkara publishes another source-based route.** Its
[Windows runtime documentation](https://github.com/tolkara/tolkara/blob/main/docs/WINDOWS.md)
links its Darwin Wine and FEX branches and reports Heroes III reaching its menu
with music on macOS without Rosetta. It relocates guest memory rather than using
the privileged low-address approach. Its documented x64 probe still faults on
hardcoded Windows shared-data addresses, and it does not demonstrate either of
the requested modern DX11 games. It is useful for a CPU-only prototype, with
further compatibility and graphics work required.

**CrossOver Preview ARM64 is another packaged baseline.** Its
[July 31 announcement](https://www.codeweavers.com/blog/mjohnson/2026/7/31/crossover-preview-the-right-to-bear-arm64-on-mac)
documents macOS FEX, ARM64 DXMT, a macOS 26.5 minimum for the ARM components,
and the need for new ARM64 bottles. That release lacked D3DMetal and had launcher
failures. Check the selected build's release notes; those July limitations are
not a verified description of every later build. A universal package can also
run Intel Wine, so verify the running process and loaded FEX modules.
[Preview access requires an active CrossOver license](https://www.codeweavers.com/preview).
No CrossOver installation was found in the usual local application folders.

**A public Wine fork provides useful engineering work, but not a complete kit.**
[gzimbric/wine-arm64-macos's build notes](https://github.com/gzimbric/wine-arm64-macos/blob/macos-arm64/docs/macos-arm64/BUILD.md)
describe Xcode 27 beta, an ARM64EC llvm-mingw toolchain and native dependencies.
Its reported setup requires an Apple-issued cross-architecture provisioning
profile and matching signing identity. Ad-hoc signing is insufficient. The
companion FEX and DXMT integration patches are external and unpublished in that
repository. Compiling its Wine branch alone therefore cannot reproduce its
reported gameplay setup.

**There is also a published Darwin FEX patch series.**
[HyperBridge/MacRunner's FEX directory](https://github.com/t0b1kent/hyperbridge/tree/main/fex)
includes a pinned base, patches, build script and output manifest. It addresses
Darwin JIT write protection and 16 KiB host pages. It is useful source material,
but does not establish compatibility with an arbitrary Wine build.
Its [repository documentation](https://github.com/t0b1kent/hyperbridge)
also identifies Wine-side changes outside that repository and experimental
correctness limits. Its reported game results are not tests of DiRT or DMC5.

None of the primary sources checked establishes a working native macOS
Wine/FEX configuration for these two games. Compatibility remains to be tested.

## Work required for a self-built runtime

1. **Select a compatible Wine/FEX/DXMT set and pin all revisions.** Obtain the
   complete Darwin integration patches before treating the build as reproducible.
   Use Apple clang for Mach-O components and an ARM64EC-capable Windows cross
   compiler for PE components. Keep Wine, wineserver and Unix modules together.
2. **Bring up Wine and FEX without graphics.** Build the ARM64 host and ARM64EC
   Windows modules, then FEX's x64 emulator and its matching Darwin companion.
   Add the x86/WoW64 emulator if the Steam client or helpers require it. Register
   emulators using that runtime's documented mechanism; names and installation
   layout vary between integrations.
3. **Resolve macOS execution constraints.** Validate low-address mappings,
   Windows 4 KiB assumptions on 16 KiB host pages, JIT W^X transitions, x18/TEB
   handling across callbacks and signals, and self-modifying code. Verify hardware
   TSO on every emulation thread when using it; otherwise validate the software
   memory-ordering mode. Follow the chosen runtime's signing/provisioning scheme.
   These are runtime engineering requirements, not Nucleon preferences.
4. **Add native graphics.** Build/package DXMT's ARM64EC/ARM64X PE libraries
   together with ARM64 Mach-O companions and a compatible Wine Metal surface
   bridge. Upstream has an [ARM64EC build configuration](https://github.com/3Shain/dxmt/blob/main/build-arm64ec.txt),
   but that alone does not supply every Darwin integration component. Test
   D3D11 device creation, shaders, readback and presentation before either game.
5. **Add media, input and Steam.** Check audio, controller input and movie
   playback, then Windows Steam login and game startup. Install only prerequisites
   the game or logs identify. Keep the existing Nucleon prefixes separate.

Do not copy Nucleon's current x86_64 Unix bridge, DXMT companion or GPTK Wine
libraries into the ARM64 runtime. FEX executes Windows guest code; it does not
make arbitrary Intel Mach-O libraries loadable in an ARM64 Wine process.

## Game-specific validation

| Game | First graphics path | Local evidence and checks |
| --- | --- | --- |
| DiRT Rally 2.0, 690790 | DX11 through native DXMT | Current Nucleon profile uses Rosetta Wine 11.18, pinned DXVK/MoltenVK, Steam helper enabled and overlay disabled. Validate a non-VR race, input, audio, saving and repeat launch. |
| Devil May Cry 5, 601150 | DX11 through native DXMT | Installed `dmc5config.ini` has `TargetPlatform=DirectX11` and `Capability=DirectX12`. Configuration is not proof of successful initialization. Validate a mission, cutscenes and skill-preview videos, controller input, saving and repeat launch. |

[DiRT's published requirements specify DirectX 11](https://store.steampowered.com/app/690790/DiRT_Rally_20/).
DMC5's existing Nucleon recipe uses a private Wine Steam-client loader redirect
and direct execution. That recipe must be revalidated on the new Wine base; the
Windows-Steam baseline does not itself require porting Nucleon's redirect.

Start with overlays, VR and optional modifications disabled to reduce the number
of components being tested. Do not require a DX12 path to establish feasibility
for these DX11 tests. An ARM64-compatible D3DMetal or VKD3D/driver combination
would be a separate experiment; existing GPTK installation is not proof of one.

## Nucleon changes after runtime feasibility

| Area | Required change |
| --- | --- |
| `crates/nucleon-core/src/wine.rs` | Describe execution mode, supported guest ABIs, emulator availability and compatible backend/bridge capabilities. Do not infer them from a Wine version string. |
| `crates/nucleon-core/src/runtime_checks.rs` | Existing host detection recognizes ARM64, but checks x86 first in universal installations. Resolve the selected execution mode. Separate guest architecture, PE library ABI and Mach-O host architecture; current `pe()` only accepts AMD64/I386. |
| `crates/nucleon-core/src/dxmt.rs` | Discover ARM64EC/ARM64X PE files and ARM64 native companions, validate the pair, and stage architecture-specific files. Avoid renaming Intel libraries to satisfy lookup paths. |
| `crates/nucleon-core/src/prefix.rs`, `steam_ntdll.rs` | Supply an ARM64 Unix Steam bridge matched to this Wine ABI, plus compatible PE entry points. Audit every native dependency. Current staging and ntdll binding explicitly require x86_64. Matching exported names alone does not prove ABI compatibility. |
| `crates/nucleon-core/src/wine_build.rs` and loader patch | Add a separate native build recipe if maintaining the runtime ourselves. Rebase DMC5's optional Steam redirect and verify cross-architecture calls/callbacks. The Wine 11.18 Rosetta patch is not a native-FEX recipe. |
| Runner/backend selection | Remove unconditional Intel Unix paths where native execution is selected. Choose only architecture-compatible backends. Log actual Wine mode, FEX and DXMT revisions and missing prerequisites. Preserve the signed native loader bundle. |

The native macOS Steam hook already has an ARM64 contract. The missing work is
the in-process Wine-side bridge and its dependencies, not recompiling that hook.

## Acceptance and performance

Native ARM64 launch, owned x64 CPU/window probes, D3D11 presentation and a native
Steam connection have passed. Next require actual gameplay, DMC5's own Steam and
rendering checks, and then Nucleon integration. Windows Steam is an alternative
compatibility route, not a completed or necessary prerequisite of the DiRT result.

Record the actual host architecture and loaded emulator/backend. Compare against
the existing Rosetta path at matching resolution, settings, scene and frame cap,
with warm shader caches and repeated runs. Measure frame times and CPU/GPU
limits as well as average FPS. Replacing both Rosetta and DXVK with FEX and DXMT
measures the whole stack; it cannot attribute a gain to FEX alone. Native Wine
does not by itself guarantee better game performance.


### DiRT Rally 2.0: first game launch (2026-10-15)

The user confirmed the title screen/menu with native ARM64 Tolkara Wine, FEX,
reference DXMT and Nucleon's existing ARM64 Unix Steam bridge. The bridge loads
native Steam's ARM64 client library; a Windows Steam install was unnecessary.
A downstream window adapter supplies DXMT's legacy table/layout and attaches
Metal to the actual game window (`attached_to_hwnd=1`). This establishes a first
presented game workload, not racing/performance or broad compatibility. The
production DiRT profile was not changed. See `tools/README.md` and
`tools/launch_dirt_fex.py` for the separate-prefix launch and local prerequisites.


### Direct3D 12 implementation (2026-10-15)

The separate DX12 recipe and build/launch tools now build Vulkan-enabled native
Wine and a pinned patched ARM64 KosmicKrisp driver. Windows Vulkan device/surface
and DX12 compute/readback/fence/triangle/presentation/resize probes pass through
FEX, including a no-draw negative control. The adapted Steam export redirect
passes a real native Steam pipe/user test with an unchanged Valve DLL on disk.
The user confirmed DMC5's title screen/menu with a 1920×1080 DX12 swapchain.
The existing Steam helper supplies the live Windows client presence required
by the game SDK, while the ARM64 bridge connects to real native Steam. The first menu run later faulted in Wine heap cleanup; a repeat DX12 run stayed
open for its 125-second test limit. Gameplay/video/stability remain unvalidated.
The driver still fails two D32 sparse-depth cases. Reproduction and limitations
are in [tools/README.md](tools/README.md#experimental-direct3d-12--dmc5).
