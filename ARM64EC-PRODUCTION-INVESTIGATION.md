# ARM64EC frontend production investigation

> Historical investigation: Python command examples below describe the tooling at
> the time of the experiment. The maintained Rust commands are in
> [tools/README.md](tools/README.md). Recorded results remain historical evidence.

Investigated 2026-10-16 on `codex/arm64-fex-dx12`, HEAD `322becd`, with the
uncommitted fixes for recommendations 4–5 present. This investigation changes
documentation only. No Wine, Steam, game, GPU probe, or installed runtime was run
or changed.

## Implementation follow-up

Following this investigation, an explicit experimental Vulkan integration was
implemented on 2026-10-16. `graphics.frontend_abi` defaults to the existing x64
behavior; `arm64ec` requires the pinned x64-guest DXVK/VKD3D set and a pairing
manifest. Inspection, selected-component validation and managed import verify
the same component/build identities, including the current native FEX companion.
A separate assembler packages the retained frontends into an unselected generation
with a distinct prefix family. Run receipts identify the frontend ABI and manifest.

The real generation passed managed import/full verification, relocation, DX11/DX12
staging and reimport. The final stricter contract also passed real-artifact
preflight/staging and rejected an unpaired override before prefix mutation.
[The integration receipt](tools/arm64ec-integration-validation.json) records these
checks and the paired hashes. No Wine, Steam, game or GPU workload was executed;
Steam registration and production selection were not changed. Graphics execution
controls, Steam/gameplay validation, performance and DXMT integration remain open.

A subsequent [installed-game validation](tools/arm64ec-game-validation.md) ran
DiRT Rally 2.0 and DMC5 with this generation, and 32-bit Braid/GoNNER with the
baseline generation. All prefix cleanup audits passed, but game failures/stalls
prevented verification of normal menu exits. Production promotion remains blocked.
The earlier no-execution statements above describe the initial integration checks.

The investigation sections below preserve the findings before this implementation.

## Decision

The existing work supports an explicit experimental integration through the
normal runner. It does not yet support making ARM64EC the production default.
The immediate work is a descriptor-backed, x64-guest, Vulkan-only runtime using
the tested DXVK/VKD3D set. Keep AMD64 as the default and retain the existing PE
checks for Steam helpers, bridges and ordinary guest modules.

Do not implement this by changing generic `runtime_checks::pe()` to accept any
64-bit image. ARM64, ARM64EC and ARM64X have distinct contracts, and compatibility
classification alone does not prove a component belongs to the validated set.

## Evidence checked

The committed summary is `tools/arm64ec-graphics-validation.json`; its narrative
is `tools/arm64ec-graphics-validation.md`. The referenced local receipts and
draw/control logs are present under `target/arm64-fex/arm64ec/runs/`.

| Recorded run | Evidence inspected | Remaining limitation |
| --- | --- | --- |
| AMD64 DXVK D3D11/Vulkan | Draw and control phases passed; loaded modules report hybrid=0 | Standalone caller only |
| AMD64 DXVK DXGI + VKD3D DX12 | Compute/readback, pixels, resize, presentation; no-draw pixel failure | Standalone caller only |
| ARM64EC DXVK D3D11/Vulkan | Draw and control phases passed; loaded modules report hybrid=1 | Steam and gameplay NOT_RUN |
| ARM64EC DXVK DXGI + VKD3D DX12 | Compute/readback, pixels, resize, presentation; no-draw pixel failure | Steam and gameplay NOT_RUN |
| ARM64EC DXMT + native Metal companion | D3D11 draw/control and wrong-component negative control recorded | Steam and gameplay NOT_RUN; separate pairing/layout work |
| ARM64EC TLS probe | Saved successful receipt agrees with the committed DLL/caller hashes | Limited private trivial TLS contract |

Current file hashing found no mismatch for the 12 unique frontend bundle files
(including DXMT's `winemetal.so`), the eight Wine boundary files named by the
graphics receipts, or the Vulkan driver against the recorded graphics summary.
This verifies the files retained locally against saved evidence; it is not a new
execution of those workloads. The current TLS receipt agrees with the committed
summary; TLS execution was not repeated.

All five graphics receipts explicitly say `steam_validation=NOT_RUN` and
`gameplay_validation=NOT_RUN`. The existing standalone Steam-route receipt says
`NATIVE_STEAM_ROUTE_PASSED`, with positive and negative controls, but it does not
validate a normal launch using this ARM64EC graphics set.

The native driver receipt still says `FAILED`. Its sparse log records two D32
depth-test failures: one-pass and two-pass cases each report 16,384 incorrect
colors and depths out of 65,536 pixels. This is a pre-existing shared driver
issue, not evidence that ARM64EC caused it. Production evaluation needs its
impact classified; the successful triangle tests do not resolve it.

Seven Python frontend-tool unit tests passed during this investigation:
`python3 -m unittest discover -s tools -p test_arm64ec_graphics.py -v`.

## Concrete integration gaps

### 1. No descriptor contract exists

`runtime.rs::{Descriptor, GraphicsPaths, ResolvedRuntime}` has no graphics
frontend ABI. Unknown descriptor fields are rejected. Therefore adding
`frontend_abi="arm64ec"` to a runtime today fails parsing.

Add a typed, defaulted graphics-specific field with values `x64` and `arm64ec`.
The initial ARM64EC contract should require `mode="arm64-fex"` and x64-only
guests, with explicit bundled graphics paths. Old descriptors remain AMD64.
Do not change prefix ownership or the game's executable ABI to encode a graphics
implementation choice.

### 2. Both launch selection and managed import reject the set

`launch.rs::GraphicsComponents::available` routes all frontend checks through a
closure calling strict `runtime_checks::pe(path, is64)`. `pe_abi()` already
distinguishes hybrid images, so this is deliberate selection policy rather than
a missing machine constant.

`managed_runtime.rs::check_runtime` invokes the same availability checks and
rejects incompatible bundled DXVK, VKD3D and DXMT before publication. Fixing
launch selection alone would leave managed installation blocked.

Introduce a graphics-specific matcher and validate every DLL that will actually
be staged, including DXGI, D3D12Core and any extra graphics DLLs present in the
bundle. Keep generic guest/Steam checks strict. Preserve failures before prefix
mutation, with a diagnostic naming the selected component and expected ABI.

The probe helper `tools/runtime-probes/graphics-abi.h` considers ARM64X native
alongside ARM64EC. That broader predicate must not become production policy:
the recorded successful loaded images are AMD64 headers with hybrid metadata.
Keep plain ARM64 and ARM64X outside the initial promotion contract and test
malformed/truncated metadata separately.

### 3. The assembler still packages the baseline graphics

`tools/arm64_fex_dx12.py::DX12.prepare_runtime` copies the reference AMD64 DXMT
bundle and baseline `self.graphics()` into a generation. It does not consume
`arm64ec/bundles/arm64ec/*` or emit a frontend ABI declaration. The frontend
experiment stages DLLs directly into its own prefix instead of using the normal
assembly/import/staging path.

Add an explicit assembly option that builds a separate generation, consumes the
pinned frontend build receipts and copies the selected DLLs into documented
descriptor paths. Include their identities in the generation inputs. Initially
omit DXMT from this ARM64EC generation instead of silently changing that backend.
The current experiment disables DXVK D3D9/D3D10; retain that capability limit.

### 4. Provenance does not yet cover the complete current FEX boundary

The graphics receipts name Wine loaders, ntdll, winevulkan and the PE emulator,
but neither their explicit boundary list nor their recorded Wine host-file map
includes `libarm64ecfex.so` or `libwow64fex.so`. Both native companions exist in
the current runtime. Existing recorded hashes therefore cannot establish that
the entire currently packaged FEX boundary was exercised as a set.

Refresh execution evidence after assembly and include the PE emulator plus its
native companion, Wine modules, Vulkan loader/driver, shader compiler, frontend
DLLs, Darwin startup/TLS objects, recipe/source identities and patch hashes.
The managed install manifest hashes package contents and audits native
dependencies; it does not independently certify that an arbitrary supplied
frontend was built with the tested Darwin CRT/TLS adaptations. Preserve build
provenance and require an explicit reviewed pairing for experimental promotion.

### 5. DXMT needs additional packaging and lookup validation

The experimental DXMT bundle is flat, containing three DLLs and `winemetal.so`.
`GraphicsComponents::discover` can retain the flat companion, but managed
`check_runtime` unconditionally selects
`bundle.root/aarch64-unix/winemetal.so`. Importing the raw flat experiment output
therefore does not satisfy that companion layout, even after an ABI matcher
change. Assemble the documented nested native-companion layout or unify resolution
before attempting this path.

The standalone DXMT probe also copies the companion beside its staged DLLs and
adjusts native search paths. Normal staging copies the graphics DLLs and uses
the resolved companion's directory in the launch environment. These are different
arrangements whose equivalence still needs a normal-runner probe. Keep the tested
native override behavior for D3D11/DXGI, winemetal's Unix-call bridge, the matched
Metal companion and the device-initialization source patch together.

### 6. Receipts and correctness coverage need normal-launch integration

`crates/nucleon-runner/src/session.rs::Receipt` records runtime, execution mode,
engine and shader cache details, but no explicit frontend ABI or complete loaded
graphics identity set. Add enough identity information to distinguish the
experiment from fallback AMD64/Wine components. A process exit or staged hash
alone cannot prove which library supplied the draw.

Repeat draw/no-draw and wrong-component controls through normal runner selection,
managed import and staging, followed by Steam helper/route controls with that
same set. Use separate prefixes/caches for comparisons. Content-based shader-cache
identity already includes selected translator/driver bytes; verify that it and
preparation receipts invalidate correctly when changing frontend sets, rather
than designing a replacement cache system without evidence it is needed.

## ABI and performance interpretation

ARM64EC is designed to interoperate with emulated x64 code in the same process;
requiring a coherent tested graphics set is an initial packaging policy, not an
ABI rule that forbids every x64 dependency. The game, Steam helper/bridge and
shader compiler can remain x64. Ordinary ARM64 DLLs are a different contract.
See [Microsoft's interoperability documentation](https://learn.microsoft.com/en-us/windows/arm/arm64ec)
and [FEX's ARM64EC build guidance](https://wiki.fex-emu.com/index.php/Development:ARM64EC).
Those sources support the architecture/build direction, not Darwin game stability.

Microsoft's current documentation describes linked ARM64EC images appearing as
AMD64 with hybrid metadata; it distinguishes the `0xA641` intermediate object
identifier from final MSVC PE images. Keep tests grounded in this pinned LLVM/Wine
toolchain's actual linked outputs rather than treating every machine identifier
accepted by an object parser as a validated production DLL.

The Darwin experiment's custom startup accessor and emulated TLS implementation
are essential pairing constraints. They cover private trivial TLS; exported TLS
and arbitrary C++ thread-local destructors are outside their documented scope.
An unrelated upstream ARM64EC binary is not interchangeable merely because its
header passes classification.

No measured FPS, frame-time or input-latency improvement is established here.
Translation savings are plausible only for code moved out of FEX; GPU-bound work,
remaining x64 game work and ABI transitions can limit the result. Benchmark a
fixed DMC5 gameplay scene after correctness validation, using matched releases,
the same Wine/FEX/driver, settings, resolution, cache policy and synchronization.
Keep TSO and shared-data emulation enabled. Repeat and alternate runs, report
median/tail frame times and variability, and measure latency directly before
claiming a latency improvement.

## Recommended next changes

1. Add the defaulted graphics ABI contract, strict graphics matcher and rejection
   tests. Use an explicitly experimental Vulkan-only x64-guest generation; keep
   old descriptors and generic PE checks unchanged.
2. Teach assembly and managed import to consume the pinned DXVK/VKD3D receipts,
   enforce selected component identities and preserve source/CRT/TLS provenance.
   Add no-execution integration tests for import, overrides, staging, rollback
   and cache separation, including mismatched DXGI/D3D12Core.
3. Refresh the complete boundary receipts and repeat positive/negative graphics
   and Steam controls through the normal runner. Record the frontend ABI and
   actual loaded identity evidence alongside each result.
4. Validate DMC5/DiRT saves, media, controllers, repeated starts and sustained
   gameplay as applicable to each backend. Investigate the earlier unexpected
   DMC5 exit and classify the sparse-depth failures. Then perform fixed-scene
   comparisons and decide whether the evidence supports production promotion.
5. Handle DXMT as a subsequent paired-layout and Metal validation change.

No production gate should be loosened as part of this documentation investigation.
