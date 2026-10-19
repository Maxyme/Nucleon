> Runtime building and assembly now live in the sibling `wine-fex` repository. Nucleon owns this installation/ABI contract; use `nucleon wine install <source> --register`. Historical developer build commands below have moved to `wine-fex-builder`.

# Runtime capabilities and launch preparation

Nucleon now resolves a selected Wine into a `ResolvedRuntime`, and prepares a
`LaunchPlan` containing its prefix, graphics components and Steam launch protocol.
Graphics availability checks, staging and the game environment use this selection.
Wine version strings do not select the execution mode.

## Native runtime descriptor

An ARM64/FEX runtime requires `nucleon-runtime.toml` at its root. The
[example descriptor](../wine-fex/assets/wine/runtime-arm64-fex.example.toml) documents the
complete layout. Ordinary Intel Wine installations can still be inferred from
their binaries. A native runtime without a descriptor fails with a configuration
error; Nucleon does not guess which emulator to use.

The descriptor records:

- Schema version, execution mode (`intel` or `arm64-fex`) and guest ABIs
  (`x64`, `x86`).
- Relative loader, wineserver, Unix and Windows library paths. Paths must exist,
  stay inside the canonical runtime root and match the selected native ABI.
  Universal loaders execute the explicitly selected slice.
- FEX emulator files for every declared guest ABI. The x64 emulator must have
  an ARM64EC PE ABI; the x86 emulator must have an ARM64 PE ABI. File-level
  checks establish a component contract, not successful CPU/game validation.
- Compatible graphics backends and optional bundled DXMT, DXVK, VKD3D, Vulkan
  ICD and Vulkan loader locations. Bundled ICDs use a relative library path to
  an architecture-compatible library inside the runtime. The Vulkan loader
  must export `vkGetInstanceProcAddr`.
- Steam bridge protocol (`bridge` or `valve-redirect`) and whether to use the
  Windows Steam helper by default. An explicit per-game helper choice wins.
- Required native prefix family and ABI (`[prefix] family = "tolkara-fex"`,
  `abi = 1`). Families contain lowercase letters, digits, `_` or `-`; the ABI
  is positive and changes when an existing prefix cannot be safely reused.
- Correctness settings `FEX_TSOENABLED=1` and, for Wine builds that need the
  downstream shared-data decoder, `NUCLEON_EMULATE_KUSD=1`. The descriptor cannot
  override runtime search paths, prefix selection or disable memory ordering.

The descriptor describes a complete runtime directory. It cannot reference
build-tree siblings using `../`. The Rust developer tools can assemble a complete local developer generation.
The managed installer imports them outside the build tree; compilation stays a developer action.

`nucleon wine status` reports resolved execution capabilities or the descriptor
error. Checking status does not initialize a prefix.

## Experimental Steam option and per-game profiles

Build the normal tools, assemble the existing prototype artifacts, then register
that complete runtime (none of these commands starts Steam or a game):

```sh
cargo build -p nucleon-cli -p nucleon-runner
../wine-fex/target/release/wine-fex-builder dx12 prepare-runtime
./target/debug/nucleon wine fex install /absolute/path/printed/by/prepare-runtime --register
./target/debug/nucleon wine fex status
```

Steam lists **Nucleon (ARM64/FEX — Experimental)** after its next restart.
Choose it explicitly in a game's Compatibility settings. Registration pins the
canonical runtime root and family/ABI; it does not change global Wine or game
mappings. Missing/changed identity is an error, with no fallback to another Wine.
Regular tool refresh preserves this selection. `nucleon wine fex unregister`
removes only this tool.

Native profiles live in `~/Library/Application Support/nucleon/game_profiles/arm64-fex/<appid>.json`,
separately from ordinary game profiles. Set options with, for example:

```sh
nucleon wine fex profile 601150 --engine auto --graphics-api auto
nucleon wine fex profile 690790 --engine dxmt --steam-protocol bridge
```

The strict JSON schema retains the existing engine, Wine, HUD, overlay, helper
and graphics-path fields, adding `graphics_api` (`auto`, `dx7`, `dx9`, `dx10`,
`dx11`, `dx12`, `vulkan`, `opengl`) and `steam_protocol` (`bridge`,
`valve-redirect`). Partial profiles fill absent fields from the embedded FEX game
presets; explicit `false` and `auto` remain choices. DMC5 defaults to the redirect
and Steam helper, with its API read from `[Render] TargetPlatform` in its INI.
DiRT defaults to DX11, the bridge protocol and helper. Both default overlay off.
Other games receive no game-specific preset. A profile's Wine selection must
agree with the dedicated tool's pinned runtime.

A graphics preference routes the selected backend; it does not change the game's
own setting. An explicit preference conflicting with DMC5's configured API is an
error. Normal Steam launches never rewrite the game INI. The diagnostic scripts
use a private profile passed through `NUCLEON_GAME_PROFILE`, bypassing one-shot
production overrides, and call the same Rust runner.

## Prebuilt publisher releases

The publisher pipeline compiles a separate experimental Wine/FEX core runtime;
users install its archive through `nucleon wine fex install` without building Wine
or FEX. See [RUNTIME-RELEASES.md](RUNTIME-RELEASES.md) for release tags, checksums,
corresponding sources, validation and scope. The core package includes WineD3D;
it does not include the local experiment's DXMT/Vulkan graphics stack.

## Install once and verify explicitly

`nucleon wine fex install <directory> --register` imports the complete runtime into
`~/Library/Application Support/nucleon/runtimes/arm64-fex/<generation>/` and points
only the experimental Steam option to it. Omitting `--register` only installs.
The runtime contains Wine, FEX, its paired graphics components, Vulkan loader and
private ICD/driver; normal Steam launches do not use the compiler, source tree,
download archives or Vulkan SDK. `prepare-runtime` remains a developer assembly
action; its output under `target/` can be removed after managed installation.

Local `.tar.gz`, `.tar.zst` and `.zip` archives, or HTTPS archive URLs, require a
checksum supplied by the caller:

```sh
nucleon wine fex install /path/to/runtime.tar.zst --sha256 <64-hex-digest> --register
nucleon wine fex verify
```

Downloads share the graphics downloader's HTTP timeouts and bounded spool.
Archives share its bounded extractor (8 GiB extracted limit for native runtimes).
Installation rejects escaping/absolute symlinks, unsupported filesystem entries,
incompatible graphics components and unresolved non-system Mach-O dependencies.
It validates the relocated package before atomic publication, preserving build
provenance and notices. A per-file manifest records SHA256, permissions and file
identities. Imports are local installations; no redistribution permission is
inferred from this manifest. Developer assembly preserves the reference DXMT
notices, Wine/FEX notices, driver notices and SDK license overview; a published
redistributable package still needs the Vulkan loader's component notices and a
complete distribution review.

Unchanged imports reuse their content generation. A damaged generation is kept
and a separate repair is published, preserving any existing process's paths.
Old generations are not automatically deleted. The runner holds a shared
generation lease through prefix cleanup; prefix family/ABI remains the separate
rule for compatible saved state.

`nucleon wine fex verify [runtime-directory]` hashes every installed file and
rechecks the native dependency closure without executing Wine, Steam or a game.
Without a path it verifies the registered tool. Normal launches use install-time
hashes only while file identities match; changed inputs are rehashed. This static
check does not validate optional dynamic loads, device behavior or gameplay.

## Prepare only changed components

Graphics preparation calculates the final DLL set first. It restores or replaces
only files whose selected contents changed, retaining the existing rollback
transaction. An unchanged repeat stage performs zero DLL writes; a missing or
modified output is repaired atomically. Backend changes preserve shared identical
DLLs and update the private Vulkan alias only when needed.

Steam staging likewise compares the final layout, including protocol-specific
Valve images and symlinks. Bound bridge variants are cached by source bridge,
selected ntdll contents, native ABI and binding schema. New prefixes reuse those
variants; unchanged prefixes skip relinking, signing and identical copies.
Cached signatures are regenerated when their tracked output changes.

Bootstrap and registry receipts include runtime inputs and settings schema.
Compatible runtime changes invalidate the relevant preparation, while unchanged
launches skip Wine registry commands. Missing bootstrap directories/registry files
trigger native prefix repair. Registry receipts track applied settings; they do
not promise that arbitrary later user edits preserve every registry value.
Readiness receipts publish only after preparation succeeds; failed preparation
retries and does not bless partial outputs.

The diagnostic launchers prefer the registered managed runtime and use file
identity for the game executable. `--debug` additionally records its full hash.
They retain their explicit `--runtime` developer override and do not extract or
hash build archives when using an installed runtime.

## Graphics selection

The runner discovers candidate bundles once and canonicalizes their component
paths. Per-game paths and explicit environment overrides retain their precedence
over bundled defaults. A launch reuses the resolved runtime rather than selecting
its architecture again during environment creation.

DXMT's Unix companion must match Wine's native ABI. Native DXMT also requires
the explicitly paired bundle declared in the runtime descriptor. AMD64 frontend DLLs can run
through FEX; ordinary ARM64 DLLs and hybrid AMD64/ARM64EC/ARM64X images are not
silently treated as equivalent AMD64 frontends. Hybrid metadata is recognized,
and unvalidated native frontend combinations are rejected. ARM64EC graphics
frontend optimization remains experimental. An explicit Vulkan-only ARM64EC
contract is available for local integration validation; the default remains the
existing x86/x64 frontend behavior.

An experimental descriptor can declare:

```toml
[graphics]
frontend_abi = "arm64ec"
frontend_manifest = "share/nucleon/arm64ec/frontend.json"
dxvk = "share/nucleon/arm64ec/dxvk"
vkd3d = "share/nucleon/arm64ec/vkd3d"
vulkan_icd = "lib/vulkan/kosmickrisp_icd.json"
vulkan_loader = "lib/vulkan/libvulkan.1.dylib"
```

Omitting `frontend_abi` selects `x64`, preserving the existing guest-specific
x86/x64 checks. `arm64ec` requires ARM64/FEX, only x64 guests, one Windows library
directory, and the pinned DXVK/VKD3D Vulkan component set. DXMT/GPTK and DX9/DX10
are outside this initial contract. The guest executable, Steam helper and bridge
retain their existing ABI requirements.

The schema-1 frontend JSON contains `frontend_abi`, `files` (runtime-relative
paths to SHA256 digests), and complete `dxvk`/`vkd3d` build receipts in `builds`.
Coverage includes the selected DLLs, their original receipts, Darwin CRT/TLS
objects/header, Wine/FEX PE and native boundaries, and Vulkan loader/driver/ICD.
Receipt sources must match the pinned frontend recipe; C and C++ options must
declare the paired emulated-TLS/thread-pointer contract. Runtime inspection and
launch selection verify identities and reject unpaired overrides before prefix
preparation. Managed import verifies the same contract after relocation.
These local provenance records establish pairing and integrity; they are not a
signed certification of arbitrary binaries or gameplay compatibility.

`wine-fex-builder frontend prepare-runtime --base-runtime /path/to/baseline`
copies an existing descriptor-backed runtime into a separate
`target/arm64-fex/arm64ec-integrated/<generation>` directory. It packages retained
ARM64EC Vulkan builds and gives them an experimental prefix family. With an
explicit baseline, assembly executes no Wine, Steam or game command and does not
register or select a Steam compatibility tool. Run receipts report the frontend
ABI and pairing-manifest path; they do not claim to identify loaded modules.

Vulkan paths require matching loader/driver/Wine components. A native FEX runtime
must declare its Vulkan loader; availability includes the native Wine Vulkan
companion. The resolved driver directories precede the loader directory to avoid
loading an SDK's different same-name driver. The legacy MoltenVK alias is private
to the prefix and points to that same resolved driver.

## Prefix and Steam setup

Intel continues to use `<compatdata>/pfx`. Native games use
`<compatdata>/nucleon/arm64-fex/<family>/pfx`, with an adjacent identity record.
Compatible runtime updates reuse that namespace; incompatible ABI reuse fails
without modifying its contents. Existing Intel prefixes are preserved. A prefix
lock spans preparation, game execution and cleanup, preventing simultaneous
launches from replacing its loaded libraries or sharing an owned wineserver.
Linked native namespace components are refused. Interrupted preparation can retry
with the recorded identity; unidentified existing native prefixes require a new
namespace. No prefix migration is performed.

Preparation validates the native and guest Steam bridge ABIs, then bootstraps
through the selected loader (`wine wineboot -i`, `wine regedit`, `wine reg`).
A separate `bin/wineboot` or `bin/regedit` executable is unnecessary. Setup
commands run in owned process groups with bounded waits and TERM/KILL cleanup.
Failures propagate; bootstrap and registry readiness markers follow success.
An ABI ownership record allows interrupted native bootstrap to retry and rejects
reuse of a prefix belonging to another execution mode. Existing untagged native
prefixes require a fresh prefix; automatic migration is not implemented.

Bridge staging uses atomic replacements and selects the native Unix companion.
For `valve-redirect`, the genuine `steamclient64.dll.valve` is preserved as the
client image, separate from the forwarding `lsteamclient.dll`. Before binding,
Nucleon checks the bridge's imports against the exact selected Wine ntdll.
ARM64 binding relinks, signs and verifies private prefix copies, with no change
to the source bridge or runtime. Intel binding retains its existing unsigned,
in-place load-command editing contract.

When required, the Steam helper receives a Windows executable path and retains
the game's individual arguments. This supplies a live Windows Steam process
identity in addition to the native session connection. The prior prototype's
`SteamAPI_IsSteamRunning`/`SteamAPI_Init` positive and negative controls remain
in `tools/runtime-probes/steam-route.c`; bridge binding alone does not replace
those end-to-end checks.

## Launch diagnostics and shader caches

Each launch writes `logs/<AppID-or-manual>/<run-id>/` under Nucleon's Application
Support directory. It contains `runner.log`, `wine.log`, `receipt.json`, and
`setup.log` when Wine setup commands run. Concurrent launches have separate files.
`nucleon logs --runner` reads the latest run; `--follow` switches to new launches.
Existing shared logs remain available as historical files.

Normal launches keep warnings/errors, engine selection, preparation summaries
and process outcomes. `NUCLEON_DEBUG=1` enables DLL/exception/process messages,
Vulkan-loader diagnostics, graphics info logs and native quit-stack tracing.
Diagnostic DXVK/DXMT files also stay in the run directory. Explicit
`NUCLEON_WINEDEBUG` channels and `NUCLEON_TRACE_PROCESS_EXIT=0|1` remain available.
`NUCLEON_WINE_LOG` redirects Wine output; `NUCLEON_RUN_RECEIPT` writes an additional
receipt for diagnostic wrappers. Both file overrides resolve relative to the
runner's working directory.

Receipts record the runtime, engine, prefix, staged-file count, effective cache
paths, outcome and separate preparation/runtime/cleanup durations. Runtime is the
supervised interval after launcher spawn, not measured gameplay or frame time.
An exit code or completed timer does not establish gameplay validity. Interrupted
runs retain their last recorded phase.

Vulkan-backed launches use persistent per-game directories beneath
`<compatdata>/nucleon/shader-cache/v1/`. Their namespaces include execution mode,
runtime family/ABI, guest architecture, engine, graphics tuning and translator/
driver contents. Moving unchanged components to a new Wine generation preserves
the same cache directory. Changed graphics inputs select a new directory while
retaining old data. Prefix repair and graphics staging do not delete caches.
Existing prototype cache locations are left intact; automatic migration is not
performed.

Explicit `VKD3D_SHADER_CACHE_PATH`, `DXVK_SHADER_CACHE_PATH` and
`MESA_SHADER_CACHE_DIR` overrides take precedence. Managed Windows translator
paths use Wine's `Z:` mapping; Mesa receives a native path.
[VKD3D cache controls](https://github.com/HansKristian-Work/vkd3d-proton/blob/v3.0.1/README.md#shader-cache)
and [DXVK cache controls](https://github.com/doitsujin/dxvk/blob/v3.1/README.md)
define the component behavior. DXMT and Metal retain their existing driver/prefix
cache handling. The pinned KosmicKrisp source does not initialize its Mesa disk
cache: Nucleon preserves its directory, but persistent Mesa cache writes require
separate driver work. Cache policy alone makes no performance claim.

The Vulkan backend no longer forces `VKD3D_CONFIG=dxr11,dxr`. Native launches
ignore inherited raw feature-forcing variables. Deliberate per-game experiments
can use `NUCLEON_VKD3D_CONFIG`, `NUCLEON_VKD3D_FEATURE_LEVEL`,
`NUCLEON_VKD3D_SHADER_MODEL`, `NUCLEON_MESA_VK_VERSION_OVERRIDE` or
`NUCLEON_MVK_CONFIG_ADVERTISE_API_VERSION`. Intel launches retain explicit raw
overrides; the `NUCLEON_` form takes precedence in both modes. Forced settings
participate in cache compatibility. Upstream describes `dxr` as forcing support
even where unsafe; it is not a default capability selection.

## Verification

Core/runner regressions cover descriptors, hybrid PE detection, graphics
selection/staging, Steam arguments, failed setup/retry, private bridge signing
and owned process cleanup. An optional integration test checks actual local
FEX images and the installed ARM64 Steam bridge without executing Wine, Steam
or a game:

```sh
NUCLEON_TEST_ARM64_RUNTIME=/path/to/built/runtime \
NUCLEON_TEST_STEAM_BRIDGE=/path/to/bridge \
cargo test -p nucleon-core --test native_runtime -- --ignored
```

These changes do not establish DMC5 gameplay stability or resolve the prototype's
two sparse-depth driver failures.

## Per-launch runtime selection

`nucleon launch <appid> --wine <exact-id-or-path>` chooses one complete runtime.
`--emulator rosetta` selects the configured Intel Wine default; `--emulator fex`
selects the registered native FEX generation (`fex-preview`). Combining the flags
constrains that Wine's mode. `--engine` remains the graphics choice.

Explicit CLI requests are resolved and pinned into the one-shot launch file,
so an already-running Steam client receives them. They supersede inherited tool
defaults for that launch, including a stale FEX tool pin. Mode-specific profiles
are selected from the effective runtime, not the tool name. Failed preflight
retains the request; no prefix is created. Incompatible guest ABIs, emulators and
separate GPTK runner requests fail rather than changing the selected Wine.

`nucleon wine list` includes managed native generations. `wine use <native-id-or-path>`
and `wine set-path <native-path>` update the FEX registration while preserving the
Intel default. No per-launch request changes either saved default. Invalid unused
saved graphics paths no longer block a selected bundled/independent route; explicit
invalid overrides still report their source. A broken experimental registration
is reported without aborting refresh of unrelated Steam tools.
