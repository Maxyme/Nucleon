# DMC5 Direct3D 12 on native ARM64 Wine/FEX

Investigation: 2026-10-15, Apple M5 Max, macOS 27.2, Vulkan SDK 1.4.363.0.

A plausible route exists, but the present prototype cannot run it by selecting
DX12. The recommended experiment is native ARM64 Wine/FEX, VKD3D-Proton and a
**patched ARM64 KosmicKrisp driver**, starting with Steamac's host-driver work.
The CPU/runtime and Steam foundations already ran DiRT; DMC5 itself has not been
launched with FEX. This investigation queried both native Vulkan drivers and
read the relevant source. It did not rebuild Wine, install a driver or launch a
game. Compatibility and performance are unproven.

```text
DMC5 x64 code → FEX → ARM64EC Wine APIs
DMC5 D3D12 calls → VKD3D-Proton + compatible DXGI
                → Wine Vulkan (ARM64 Unix side)
                → ARM64 Vulkan loader → patched KosmicKrisp → Metal
```

## Evidence already available

- Tolkara Wine/FEX passes the recorded CPU/window/shared-data tests; DiRT reached
  its menu through DXMT and the real native Steam client, confirmed by the user.
- The prototype recipe explicitly disables Vulkan. Its working DXMT DLLs and
  Metal window adapter implement a different graphics path.
- The installed GPTK 4 beta 3 D3DMetal framework is x86_64 Mach-O. FEX translates
  Windows guest code, not Intel Mach-O libraries inside an ARM64 process.
- The SDK includes an ARM64 KosmicKrisp driver and a universal Vulkan loader.
  The other KosmicKrisp copy under `target/bloodborne-vulkan` is x86_64 and is
  unsuitable for this runtime.
- VKD3D-Proton v3.0.1 x64 Windows DLLs are locally installed. They can be the
  initial translated frontend; building ARM64EC versions is an optimization
  milestone, not a prerequisite to investigating the Vulkan boundary.

The native queries explicitly used `arch -arm64` and selected one ICD at a time.
Raw results and a structured summary are under
`target/arm64-fex/dx12-investigation/`. The summary includes actual library hashes.
The SDK KosmicKrisp identifies itself as `git-269a955c89`; MoltenVK as 1.4.2.

## Driver findings: Vulkan version is insufficient

| Capability measured here | SDK KosmicKrisp ARM64 | SDK MoltenVK ARM64 | Consequence |
| --- | --- | --- | --- |
| Reported Vulkan version | 1.4.363 | 1.4.357 | Both meet the headline API version |
| Robust buffer access 2 / null descriptors | Both true | Both false | MoltenVK fails explicit VKD3D v3.0.1 device requirements |
| Robust image access 2 | True | True | Present |
| Push descriptors / shader draw parameters / mirrored clamp | Present | Present | Present |
| Sampled-image descriptors after binding | 1,048,576 | 1,000,000 | Large descriptor heaps are available |
| Mutable descriptor types | Present | Absent | KosmicKrisp has the preferable descriptor path |
| Transform-feedback extension | Absent | Absent | VKD3D v3.0.1 explicitly requires transform-feedback queries |
| Single-texel uniform/storage buffer alignment | False; 16-byte alignment | False; 16-byte alignment | VKD3D v3.0.1 rejects this combination |
| Geometry shaders | False | False | Pipeline compatibility risk; DMC5's actual shader use remains untested |
| Sparse binding / buffer and 2D image residency | False | False | Affects tiled resources and higher D3D feature levels |
| Descriptor buffers / image-view minimum LOD | Absent | Absent | Additional capability/performance gaps; not equivalent to every hard requirement |

The first source-level blockers are stronger than a generic Vulkan-version
warning: VKD3D's `vkd3d_init_device_caps()` returns `E_INVALIDARG` for missing
transform-feedback queries or single-texel alignment. It separately rejects
missing robustness features. These are predictions from matching source and
measured capabilities, **not observed Windows D3D12 device-creation attempts**.
The downloaded v3.0.1 source files were checked against GitHub's Git blob hashes.

Sources: [VKD3D requirements](https://github.com/HansKristian-Work/vkd3d-proton/blob/v3.0.1/README.md),
[device checks](https://github.com/HansKristian-Work/vkd3d-proton/blob/v3.0.1/libs/vkd3d/device.c),
[Mesa KosmicKrisp documentation](https://docs.mesa3d.org/drivers/kosmickrisp.html).

## Most useful external work: Steamac's driver, without its VM

Inspected Steamac revision: `6ef3992d869e41291bb2841bef2e71ae318d2a02`.
Its separate host KosmicKrisp build carries geometry-shader and transform-feedback
patches, single-texel alignment work, and sparse-resource additions. These
address the concrete SDK gaps above. The maintainer reports DX12 workloads on
that patched stack; this is not evidence of DMC5 on our native Wine/FEX runtime.

We can evaluate that ARM64 macOS driver through the normal Vulkan loader and an
ICD manifest. The driver itself does not require the Linux VM, Venus,
virglrenderer or libkrun. Host-driver suitability still needs direct tests:
Steamac's game results use a different execution/presentation stack. Its build
script has package-install and source-reset steps; extract a pinned downstream
recipe into a new work directory rather than running it over existing sources.

Upstream geometry/transform-feedback work remains experimental. The build notes
identify unresolved strip-geometry/overflow-query cases. Preserve these limits;
do not advertise unsupported capabilities just to make device creation pass.

Sources: [pinned host-driver build](https://github.com/fxgl/steamac/blob/6ef3992d869e41291bb2841bef2e71ae318d2a02/host/kosmickrisp/build.sh),
[pinned driver status](https://github.com/fxgl/steamac/blob/6ef3992d869e41291bb2841bef2e71ae318d2a02/README.md#vulkan-driver).

## Required implementation, in order

### 1. Establish the patched driver independently

Build/package the pinned ARM64 KosmicKrisp patch set with a provenance receipt,
license notices, a private ICD manifest and valid local signing. Retain the SDK
build as a control. Check real transform-feedback queries, texel-buffer offsets,
geometry shaders, synchronization and sparse operations with GPU readback. The
existing native enumeration results are only capability reporting.

Prefer a known patch set first. Reimplementing geometry shaders, stream output
and sparse resources ourselves would turn this into a graphics-driver project.

### 2. Add Vulkan to a separate Tolkara Wine build

Give the prototype a Vulkan build option and a separate build/runtime directory.
Remove `--without-vulkan` for that variant and make configure discover the ARM64
Khronos loader using explicit SDK include/library paths. Current build setup
suppresses general pkg-config discovery, so removing the flag alone is not enough.
Record `SONAME_LIBVULKAN` and the exact selected loader. Preserve the proven DX11
runtime and prefix.

Rebuild the matching `winevulkan`, `vulkan-1`, Unix `win32u` and `winemac` components
as one runtime. Existing generated Wine thunks list the required descriptor,
robustness, transform-feedback and Metal-surface extensions; their actual ARM64EC
calling boundary and feature-chain marshalling still need execution tests.

Use **the Vulkan loader**, with `VK_DRIVER_FILES` pointing to KosmicKrisp. Do not
rename the KosmicKrisp ICD library to libMoltenVK. In this Wine fork, the Mac driver
looks up both `vkCreateMacOSSurfaceMVK` and `vkCreateMetalSurfaceEXT`; the SDK loader
exports both, while the KosmicKrisp ICD is not a direct drop-in for that lookup.
Validate Win32 surface creation, presentation support and swapchain images.
The working DXMT window adapter is not proof of Wine's Vulkan presentation path.

### 3. Pair VKD3D with Vulkan-compatible DXGI

Stage `d3d12.dll`, `d3d12core.dll` and a compatible DXVK `dxgi.dll` in a new prefix.
Remove DXMT's DXGI/graphics overrides in that prefix; the Metal-specific DXMT DXGI
cannot be assumed to consume VKD3D's Vulkan device/queue. Audit the selected DXGI's
VKD3D swapchain interop, loader requirements and Wine-version requirements.

Begin with x64 Windows components translated by FEX, then cross-build VKD3D and
DXGI as ARM64EC after correctness is established. Ordinary Windows ARM64 is not
a substitute for ARM64EC when the caller is the x64 game. Keep every Unix loader,
ICD and companion ARM64. FEX documents ARM64EC builds for both DXVK and VKD3D.

Add an owned x64 D3D12 probe: device/queue creation at the actual supported feature
levels, upload/compute/readback, triangle, fences, presentation and resize. Include
negative checks for missing drawing/incorrect readback. Use shader model/feature
queries rather than forcing `VKD3D_FEATURE_LEVEL`. DX12 API selection does not
mean a game necessarily requires D3D feature level 12_0 or ray tracing.

Source: [FEX ARM64EC graphics build guidance](https://wiki.fex-emu.com/index.php/Development:ARM64EC).

### 4. Revalidate DMC5's Steam arrangement under FEX

This is independent of DX12. DiRT's successful alias layout is not sufficient
proof for DMC5. The previous confirmed DMC5 recipe preserved Valve's Windows
`steamclient64.dll` on disk and redirected its mapped exports into `lsteamclient`,
then launched the game directly. Aliasing the bridge over Valve's DLL failed
before graphics. See [the original investigation](dmc5-native-steam-investigation.md).

Rebase that opt-in loader behavior onto the pinned Tolkara source. Validate
AMD64 Valve exports, x64/ARM64EC bridge entry points, callbacks, exception unwind,
page protection and FEX's translated-code invalidation. Our existing 12-byte
x64 jump patch cannot be assumed correct at every ARM64EC boundary. The compiler
defines both `__x86_64__` and `__arm64ec__`, so its `#ifdef __x86_64__` guard alone
is not an architecture audit. Bind private ARM64 Unix bridge copies to the exact
selected Wine ntdll, as in the successful DiRT experiment.

First establish a DMC5 DX11/FEX launch using that Steam route. Then change only
its isolated game configuration to DX12. Use a controlled configuration copy or
a backed-up, restored INI; the installed file is currently `TargetPlatform=DirectX11`
and `Capability=DirectX12`. Require logs proving VKD3D device/queue/presentation,
so automatic DX11 fallback cannot masquerade as a DX12 success.

### 5. Validate the game, then expose it in Nucleon

Require menu, a playable mission, shader-heavy effects, audio, controllers,
saving/relaunch and video playback. The prototype disables GStreamer/FFmpeg;
DMC5 media playback may therefore need a separate dependency pass. Investigate
observed failures rather than treating every startup issue as a Vulkan bug.

Only after those gates, teach runtime discovery/staging about execution mode,
ARM64EC/ARM64X PE libraries, native ARM64 companions, matching loader/ICD and the
DMC5-specific Steam recipe. Current `steam_ntdll.rs` explicitly binds x86_64;
backend paths also contain x86_64 Unix directories. `runtime_checks::vulkan()`
checks API version and architecture, not the feature requirements measured here.
The KosmicKrisp backend also adds DXR flags whenever VKD3D is installed; these
should not be unconditional requirements for this DMC5 experiment.

## Alternatives and scope

- **GPTK/D3DMetal:** the installed Intel host framework is incompatible with this
  native process. A compatible ARM64 vendor implementation would be a separate
  prerequisite; translating its Windows PE frontend does not fix that.
- **Current DXMT:** its documented supported APIs are D3D10/11. Extending it into
  a complete D3D12 renderer is substantially larger than using VKD3D.
- **Patched MoltenVK:** Steamac and MetalSharp publish relevant downstream work.
  MetalSharp's VKD3D macOS project documents a universal native driver but an
  x86_64 Wine/Rosetta validation route. Treat that as an alternative driver and
  DXGI reference, not a verified ARM64/FEX runtime or DMC5 result.

Sources: [DXMT scope](https://github.com/3Shain/dxmt),
[MetalSharp's stated runtime and validation scope](https://github.com/metalsharp/VKD3D-Proton-MacOS/blob/main/docs/Final.md).

The next useful milestone is a **patched-driver capability/execution gate plus
an x64 D3D12 GPU probe through Vulkan-enabled ARM64 Wine**. That separates driver
work from DMC5's Steam/CPU behavior. ARM64EC graphics optimization and Nucleon UI
integration can follow. No reliable completion estimate follows from the current
evidence: the build integration is bounded, while shader/driver and game-specific
compatibility remain open-ended.
