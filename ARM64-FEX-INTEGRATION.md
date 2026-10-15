# Integrating the ARM64/FEX prototype into Nucleon

Investigation: 2026-10-15, against `44e07ca`. Source inspection and existing
local test receipts only; no game was launched or runtime selection changed.

The useful next step is an experimental runtime supported by Nucleon's normal
runner. FEX should be a property of the selected Wine runtime, independent of
the graphics backend. DMC5 would use ARM64/FEX + VKD3D/DXGI + KosmicKrisp;
DiRT would use ARM64/FEX + DXMT. Both should launch from Steam without a
game-specific Python launcher.

## What the current code establishes

- `wine.rs:19` describes a runtime by name, root and version. It has no execution
  mode, emulator, native-library ABI, graphics capability or compatibility status.
- `runtime_checks.rs:38` prefers Intel ntdll when both Unix architectures exist.
  `pe()` accepts only AMD64/I386. These checks cannot describe an explicitly
  selected native execution mode or ARM64EC/ARM64X frontend.
- `runner.rs:1806` partly recognizes ARM64 directories, but omits
  `aarch64-windows` from its normal DLL paths and always sets Rosetta options.
  Backend/helper environment builders still construct Intel Unix paths.
- `prefix.rs:8,198` stages the existing bridge and `steam_ntdll.rs:60` requires
  Intel Mach-O. The latter also rejects signed inputs; supporting the existing
  signed ARM64 bridge requires a separate private-copy signing path.
- `detector.rs:137` discovers candidate graphics packages, while backend staging
  and environment construction discover them again. Availability and actual
  launch can therefore resolve different packages.
- The normal runner already provides per-game Wine/graphics/Steam-helper
  overrides, backend transactions, process-group ownership and prefix-scoped
  wineserver cleanup. Steam tool registration can already pin a Wine path.
  These should be extended and reused.
- `arm64_fex_dx12.py:233,303` repeats graphics archive inspection/extraction,
  prefix initialization, DLL copies, bridge relinking/signing and three Wine
  registry commands on each launch. `launch_dirt_fex.py` duplicates much of it.
- The installed DMC5 executable is 130.5 MiB and is hashed for every prototype
  launch. The two graphics archives total about 22.1 MiB; their extracted DLLs
  total about 11.0 MiB. These are avoidable repeated reads/writes, not measured
  launch-time savings.
- The last 125-second diagnostic run produced a 6.2 MiB log. The prototype
  enables DLL/Vulkan-loader diagnostics even without `--debug`.

## Recommended implementation order

### 1. Describe runtime capabilities and resolve one launch plan

Add a versioned runtime descriptor, separate from the build recipe and human
Wine version. Record the selected host execution mode, supported guest ABIs,
Wine PE/Unix directories, emulator files, required environment, matching bridge
variant and compatible graphics components. Store paths relative to the runtime
root; validate binary architecture and required files rather than trusting names.

Resolve runtime, prefix, graphics DLLs, Vulkan loader/ICD, bridge and Steam launch
protocol into one `LaunchPlan`. Use those exact objects for preflight, staging,
environment creation and execution. Keep existing engine names as user-facing
choices; avoid adding separate FEX variants of every graphics engine.

For ordinary imported Wine, derive conservative capabilities. For the packaged
FEX runtime, require the descriptor and matching emulator. ARM64EC and ARM64X
validation must account for hybrid PE metadata and calling conventions, rather
than accepting any ARM64 PE as an x64 replacement.

Main files: `wine.rs`, `runtime_checks.rs`, `detector.rs`, `runner.rs`,
`nucleon-runner/src/lib.rs`. Scope: medium architectural change.

### 2. Make prefix and Steam preparation architecture-aware

Pass the resolved runtime/bridge into prefix preparation. Select ARM64 Unix
libraries with translated AMD64 PE bridge entry points for the current prototype.
Retain the genuine Valve client image for the DMC5 redirect protocol. Use the
existing import/export compatibility checks against the exact selected ntdll,
then patch and sign a private bridge copy; never edit the installed source bridge.

Treat Steam helper use as a launch-protocol requirement, independent of game
bitness or graphics API. DMC5's working path needs the helper's live Windows
process identity and a Windows executable path. A successful pipe connection
alone did not establish Steam SDK readiness. Preserve explicit per-game choices
and validate both `SteamAPI_IsSteamRunning` and `SteamAPI_Init` with the existing
probe and negative controls.

Bootstrap through the selected Wine loader when a standalone `bin/wineboot` is
absent. Propagate bootstrap/registry failures and write readiness markers only
after success; current prefix setup ignores several command results. Prepare
before staging native overrides so Wine updates cannot replace them afterward.

Main files: `prefix.rs`, `steam_ntdll.rs`, runner environment helpers.
Scope: medium; required before merely registering this Wine can work reliably.

### 3. Expose an experimental Steam option through the normal runner

Register one explicit option such as **Nucleon (ARM64/FEX — Experimental)**,
pinning runtime identity while allowing compatible graphics selection. Keep it
opt-in until gameplay and stability have been established. The existing automatic
tool can use its capabilities when the user explicitly selects this runtime.

Use the existing per-game profile mechanism for runtime, backend, overlay and
Steam protocol. Add only missing structured fields, including an optional
game-API preference; DMC5 imports do not establish its configured rendering API.
The existing schema denies unknown fields, so profile changes require a deliberate
schema/parser update rather than dropping arbitrary environment keys into JSON.

Derive a separate FEX prefix namespace beneath Steam's provided compatdata root.
Record runtime family/ABI compatibility there, retain the prefix for compatible
updates, and reject incompatible reuse. Do not open an existing Rosetta prefix
with native Wine merely because the same AppID selected another Steam tool.
Hold a prefix launch lock through process cleanup so another launch cannot
replace mapped libraries or kill a shared wineserver.

Move the two launchers' shared preparation into core/runner and keep the Python
tools for builds and diagnostics. Reuse `OwnedChild` and existing termination
handling for all owned bootstrap helpers as well as the game.

Main files: `steam/mod.rs`, `vdf.rs`, `paths.rs`, runner profile/launch handling.
Scope: medium after steps 1–2.

### 4. Install once; prepare only when inputs change

Import/package the complete working runtime into Nucleon's managed storage,
outside `target/`. Include Wine, FEX, the matched graphics components, Vulkan
loader, private ICD and provenance/licenses. Preserve the driver's actual
dependency closure and audit redistribution terms for packaged inputs; the
developer SDK and game-supplied DLLs should not become silent runtime prerequisites.
Leave compiling sources as a developer action.

Reuse Nucleon's immutable-generation publication and shared download/extraction
code, extending archive formats/limits where the new package requires it.
Resolve a generation once per launch and retain it while in use. A relative ICD
and controlled library lookup must resolve the packaged driver: the experiment
already encountered a same-name SDK driver overriding the patched build.

Cache prepared prefix state by runtime/bridge/graphics identities and settings
schema. Cache signed bridge variants by source bridge hash, selected ntdll hash
and native ABI. On an unchanged launch, skip extraction, relinking, signing,
registry setup and identical DLL replacement. Repair missing or changed files
atomically and publish readiness only after all preparation succeeds.

Use install-time hashes plus file-identity checks for ordinary launches; keep a
full verification action for diagnosis. This removes repeated work without
assuming an existing marker proves files are intact.

Main files: `downloads.rs`, `wine_build.rs`, `backend_files.rs`, prefix staging.
Scope: medium; the clearest startup/I/O improvement.

### 5. Keep normal launches quiet and preserve compatible shader caches

Make full DLL, Vulkan-loader and quit-stack traces diagnostic-only. Normal
launches should retain warnings/errors and a compact per-run receipt. Use
AppID/run-specific logs instead of overwriting the shared `wine.log`.

The prototype already keeps Mesa/VKD3D caches per prefix; retain that behavior
when integrating it. Add explicit per-game VKD3D cache paths to production and
keep caches across compatible runtime updates. Namespace by relevant translator,
driver and ABI compatibility, without invalidating everything on every unrelated
Wine diagnostic rebuild. VKD3D documents its cache as reducing shader conversion
work, load time and stutter; the actual benefit here still needs measurement.
[VKD3D cache documentation](https://github.com/HansKristian-Work/vkd3d-proton/blob/master/README.md#shader-cache).

Do not inherit the production Vulkan backend's blanket `VKD3D_CONFIG=dxr11,dxr`
or feature-level overrides. Leave feature enablement to the validated stack unless
a measured game-specific workaround is required. Upstream explicitly describes
`dxr` as forcing support even when considered unsafe.
[VKD3D environment documentation](https://github.com/HansKristian-Work/vkd3d-proton/blob/master/README.md#environment-variables).

Record preparation duration separately from game runtime. Compare repeat launch
time, cold/warm shader behavior, CPU/GPU time, median and tail frame times in a
fixed gameplay scene. Neither the existing menu test nor less setup code proves
an FPS improvement. Scope: small to medium; do before performance comparisons.

### 6. Experiment with ARM64EC graphics frontends

Wine's Unix modules and the Vulkan driver are already native ARM64. The current
VKD3D and DXGI Windows DLLs are AMD64 and therefore translated by FEX. Building
matched ARM64EC VKD3D + DXGI is a plausible way to reduce CPU translation work
on the graphics submission path. DXMT ARM64EC/ARM64X is a corresponding DiRT
experiment. FEX documents ARM64EC builds of DXVK/VKD3D, and DXMT supplies an
ARM64EC cross file. These establish build directions, not Darwin compatibility
or an expected speedup.
[FEX ARM64EC documentation](https://wiki.fex-emu.com/index.php/Development:ARM64EC),
[DXMT build configuration](https://github.com/3Shain/dxmt/blob/main/build-arm64ec.txt).

Build a coherent PE/Unix set against the pinned Wine Vulkan/Metal boundary;
mixing one native DLL into the current set is insufficient. Re-run shader,
compute, readback, presentation/resize, Steam and game tests before comparing
frame times with the existing AMD64 frontends. Keep software memory ordering
and shared-data emulation enabled; changing those is a separate correctness
experiment. Persistent FEX code caching is also separate research: this pinned
tree defaults disk caching off and its Windows image-cache code contains a
configuration-ID TODO. Scope: large, experimental; performance is unmeasured.

## Reliability work that should travel with integration

- Journal temporary game-configuration changes and recover them after a crash.
  The prototype's exact INI restoration intentionally discards in-game changes;
  a normal product flow should expose the API choice and preserve unrelated user
  edits. Lock by the actual configuration path, since two prefixes can share it.
- Preserve separate evidence for device creation, presentation, user-confirmed
  menu, gameplay, expected timer shutdown and unexpected exits. The first menu
  run quit and faulted in heap teardown; a later 125-second run remained open.
  Integration cannot label that game stable yet.
- Keep the two sparse-depth driver failures visible in runtime health. Associate
  validation receipts with exact binaries and test context instead of treating
  “installed” as “validated.” Re-run invalidated checks on explicit validation
  or installation, not on every game start.
- The prototype Wine recipe disables GStreamer/FFmpeg and SDL. Validate movies,
  audio, controllers, saves and repeat gameplay before promoting this runtime;
  add required native dependencies based on those results. The switches alone
  do not prove why DMC5 quit.

The first useful delivery is steps **1–3**, followed by **4–5** for repeat-launch
efficiency. Step **6** is the main CPU-performance experiment after the launch
path is integrated and measurable.

## Implementation follow-up

Steps 1–4 are implemented in core and the runner: explicit runtime descriptors,
a shared launch plan, pinned graphics paths, architecture-aware bridge staging,
private ARM64 relinking/signing and checked prefix setup. The experimental Steam
tool pins its native runtime, uses scoped profiles, and keeps a family/ABI-checked
prefix namespace locked through cleanup. The DiRT and DMC5 diagnostic launchers
now delegate setup and execution to the normal runner. DMC5's temporary INI change
is locked by the actual configuration path and restored after the run.

See [RUNTIME-DESCRIPTORS.md](RUNTIME-DESCRIPTORS.md) for registration, profiles and
validation. Developer generations are assembled from existing local artifacts and can be
imported into retained managed storage. Managed runtime installation and
preparation caches are now available.
Step 5 remains the follow-up for logs, shader-cache policy and performance measurement. Existing
game selections and global Wine remain unchanged. The new runner flow has not
received a new gameplay validation; the earlier DMC5 exit and sparse-depth failures
remain unresolved.
