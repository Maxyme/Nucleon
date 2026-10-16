# ARM64EC graphics frontend experiment

Tested locally on 2026-10-15 with Apple M5 Max, the pinned ARM64 Tolkara Wine/FEX
DX12 runtime and patched KosmicKrisp driver. No Steam or game was launched.
The installed experimental tool and normal runner's hybrid-frontend rejection
remain unchanged. This is a developer experiment, not a game compatibility or
performance certification.

| Set | Result |
| --- | --- |
| ARM64EC VKD3D 3.0.1 + ARM64EC DXVK 3.1 DXGI | DX12 shader compilation, compute/readback/fence, triangle pixels, presentation and resize passed |
| AMD64 builds of the same releases | Same DX12 checks passed |
| ARM64EC DXVK 3.1 D3D11 + DXGI | GPU copy/readback, shader compilation and triangle pixels passed |
| AMD64 DXVK 3.1 | Same D3D11 checks passed |
| ARM64EC CRT/TLS support from AMD64 callers | 32 threads × 10,000 calls; initialization, alignment, isolation, thread-exit cleanup and sharing across fiber switches/deletion passed |
| ARM64EC DXMT + native Metal companion | GPU copy/readback, shader compilation, triangle pixels and negative controls passed with normal Wine warnings/heap checks |

All four GPU runs also passed a no-draw negative control: removing drawing
produced the expected pixel mismatch and exit 21. Successful process exit alone
is insufficient. Probe output identifies the actual loaded DLLs and their hybrid
metadata. Wine rewrites their mapped machine headers to AMD64; a nonzero CHPE
metadata pointer distinguishes the loaded native frontends from the AMD64 controls.
Compact component hashes and outcomes are recorded in
[arm64ec-graphics-validation.json](arm64ec-graphics-validation.json). Full local
logs and per-run receipts remain under `target/arm64-fex/arm64ec/runs/`.

## Darwin adaptations

Unmodified ARM64EC VKD3D compiled but faulted during `_CRT_INIT`: MinGW read
`[x18 + 8]` for the thread's stack base. This Darwin Wine/FEX build cannot rely on
`x18` surviving kernel transitions and uses pthread TSD offset `0x17f8` instead.
The experiment rebuilds the pinned MinGW `dllcrt2.o` with the same accessor as
the paired Wine/FEX ABI. It checks both build configurations before doing so.
It does not edit the shared toolchain or patch generated machine instructions.

That startup fix alone exposed LLVM's generated static TLS reads from
`[x18 + 0x58]`. The frontends now compile with `-femulated-tls`, linked to a private
Win32 TLS implementation. Values retain their initializer and alignment, and
PE thread-detach callbacks release worker-thread allocations. True thread TLS
also preserves sharing between fibers on one thread. The source has only private,
trivial TLS values; exported TLS across DLLs and arbitrary C++ thread-local
destructors are outside this implementation's contract. Do not treat it as a
general ARM64EC CRT replacement.

The native path keeps FEX software memory ordering and shared-data emulation
enabled. Shader caches and prefixes are isolated from the installed runtime.
Build receipts retain source/submodule identities, compiler provenance, compiler
options, CRT/TLS object hashes and DLL hashes. GPU receipts include the Wine,
emulator, Vulkan boundary, driver, probe and shader-compiler identities.

## DXMT build and initialization fixes

Installed Metal Toolchain 27A266a (compiler 32023.921) and side-by-side Homebrew
LLVM 15.0.7. The prerequisite check reports both repairs before compilation.
Preserving Homebrew's `opt` alias enables upstream's static zstd/unwind linkage;
resolving it to `Cellar` had skipped that dependency branch.

DXMT's D3D11/DXGI DLLs must be native overrides; marking them Wine builtins made
Wine load its own D3D11 first. Only `winemetal` keeps the builtin Unix-call bridge.
Its newly built native companion is staged beside it. The actual loaded images
are checked against build timestamp, image size and checksum, in addition to
hybrid ABI metadata and file hashes. Deliberately supplying a wrong D3D11 image
identity fails with exit 6 before pixel testing, rather than accepting a fallback.

Normal warnings enable Wine heap checking. That exposed a pinned DXMT source bug:
`ClearUAV` constructed compute pipelines through `ArgumentEncodingContext` before
its `device_` member was initialized, passing heap-fill `0xbaadf00dbaadf00d` as a
Metal object. The recorded source patch moves the member and its initialization
before those helpers. Debug-only success did not establish this correction;
the final normal warning-mode run passed both positive and negative controls.

These results cover the paired ARM64EC Windows DLLs and native ARM64 Metal
companion built together. The existing paired AMD64 DXMT path remains available.

## Remaining validation

Before exposing ARM64EC frontends through Nucleon's normal launch plan, add an
explicit descriptor contract for the tested frontend ABI and component set,
repeat Steam positive/negative controls and validate DMC5/DiRT saves, video,
controllers, repeated launches and sustained gameplay. Keep the earlier DMC5
unexpected exit and the two sparse-depth driver failures unresolved until tested.
These smoke tests make no FPS, frame-time or startup-speed claim. Compare those
only in the same fixed gameplay scene after correctness validation.

Reproduction commands are in [tools/README.md](README.md#arm64ec-graphics-experiments).
The build direction follows the
[FEX ARM64EC frontend instructions](https://wiki.fex-emu.com/index.php/Development:ARM64EC);
Darwin's additional CRT/TLS constraints were established by the local failures
and successful controls described above.
