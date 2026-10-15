# Runtime capabilities and launch preparation

Nucleon now resolves a selected Wine into a `ResolvedRuntime`, and prepares a
`LaunchPlan` containing its prefix, graphics components and Steam launch protocol.
Graphics availability checks, staging and the game environment use this selection.
Wine version strings do not select the execution mode.

## Native runtime descriptor

An ARM64/FEX runtime requires `nucleon-runtime.toml` at its root. The
[example descriptor](assets/wine/runtime-arm64-fex.example.toml) documents the
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
build-tree siblings using `../`. The Python build tools can assemble a complete local developer generation.
The managed installer imports them outside the build tree; compilation stays a developer action.

`nucleon wine status` reports resolved execution capabilities or the descriptor
error. Checking status does not initialize a prefix.

## Experimental Steam option and per-game profiles

Build the normal tools, assemble the existing prototype artifacts, then register
that complete runtime (none of these commands starts Steam or a game):

```sh
cargo build -p nucleon-cli -p nucleon-runner
python3 tools/arm64_fex_dx12.py prepare-runtime
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
frontend optimization remains a separate experiment.

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
