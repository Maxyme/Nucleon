# Managed temporal upscaling

Nucleon can prepare OptiScaler next to a game's rendering EXE and select its
output on each launch. This works through the shared Steam/standalone launch
path. Providers and runtime combinations are **experimental**; preparation and
a loaded library do not establish successful in-game reconstruction.

Choices are `game` (default), `fsr2` (2.2.1), `fsr3.1`, `xess`, `metalfx`,
`fsr4-native-lite`, `fsr4-native` and `fsr4.1.2`. The existing native choices
retain **4.1.1**, with `lossy` for Lite and `exact-fast` for full quality.
`fsr4.1.2` selects the separate **4.1.2 INT8 Lite** model (`int8-lite`).
The native project's optional Lite 2 weights are not installed by this integration.

## Prepare dependencies without launching a game

Build the CLI with `cargo build --release -p nucleon-cli`. The examples use
`nucleon`; from a checkout, use `target/release/nucleon`.

For ordinary FSR2/3.1 or experimental XeSS output:

```sh
nucleon upscaler install stock
```

For GPTK MetalFX/native FSR4:

```sh
nucleon upscaler install metalfx
nucleon upscaler install bridge --wine /path/to/complete/gptk/wine
nucleon upscaler install native --wine /path/to/complete/gptk/wine
```

For FSR 4.1.2, install `native412` instead of `native`. For example:

```sh
nucleon upscaler install native412 --wine /path/to/complete/gptk/wine
nucleon upscaler prepare /games/Game/Game.exe --backend fsr4.1.2 --input dlss \
  --wine /path/to/complete/gptk/wine --engine wine --graphics-api dx12 --dry-run
```

After quitting the game, repeat preparation without `--dry-run`, then launch:

```sh
nucleon run /games/Game/Game.exe --wine /path/to/complete/gptk/wine \
  --engine wine --graphics-api dx12 --upscaler fsr4.1.2
```

Enable DLSS in the game. This example requires an eligible x64 DX12 game with
DLSS input and a complete matching Intel GPTK runtime; it does not apply to an
arbitrary game. The 4.1.2 provider verifies its source and the upstream v2.2 ZIP,
then imports only the model payload from that ZIP. No separate AMD driver DLL
is required. `--archive /path/fsr4-mac-v2.2-crossover.zip` reuses the exact pinned
ZIP. Its provider generation is separate from 4.1.1, so both can remain installed.

The native commands build and validate owned synthetic Metal frames: Lite and
full quality for 4.1.1, and INT8 Lite for 4.1.2. They never execute Wine or the game. They require
Python 3.11+, Rosetta, Xcode with its Metal Toolchain, and `dxc`, `spirv-cross`
and `spirv-val` on PATH. The selected Intel GPTK bundle must contain its matched
`lib/external/D3DMetal.framework` and Windows MetalFX bridge. Original framework
bytes are checked before and after building a private copy.

Stock/MetalFX archives are verified and extracted in Rust; they do not require
Python. Provider updates are built and validated in a separate folder, then
published atomically. Failed attempts leave the previous generation intact, and
a retry can repair an incomplete generation. Existing provider identities remain
compatible. Rust handles native source downloads, pinned-file verification,
compatibility patches, build sequencing and validation reports. Python is still
required by the pinned upstream FSR4 build and shader-generation tools.

The bridge command also needs `x86_64-w64-mingw32-gcc`. It builds a small x64
export-forwarding DLL from the selected runtime's NGX exports. The actual vendor
bridge retains its `nvngx-on-metalfx.dll` filename. A separate Wine builtin entry
named `nvngx.dll` satisfies the game's static loader; OptiScaler intercepts that
load and sends calls through the forwarding DLL under `OptiScaler/nucleon-metalfx`.
All game-local files participate in the same ownership and rollback transaction.

Release archives and source revisions are pinned in
[dependencies.toml](assets/upscalers/dependencies.toml). Use `--archive /file.7z`
with stock/metalfx to reuse a matching archive, or `--source /verified/fsr/tree`
with native/native412 to reuse source matching the embedded Git-blob source lock.
An unexpected checksum is rejected. Installation retains dependency notices;
GPTK components are copied locally from the selected runtime, not downloaded or
redistributed by Nucleon.

Preparation does not initialize a prefix, alter a registry, modify an original
runtime, change Heroic preferences, obtain store authentication, or start a game.
Ordinary launches require installed dependencies and a prepared game binding;
they do not install providers. The native renderer can still generate/cache
resolution-specific Metal shaders during first use.

## Bind the rendering EXE

The rendering EXE may be different from the store/bootstrap launcher. For Unreal
games it is usually under `Project/Binaries/Win64/`, rather than the root EXE.
Provide its location explicitly when detection is ambiguous:

```sh
nucleon upscaler prepare /games/Game/Launcher.exe --backend fsr3.1 \
  --renderer Project/Binaries/Win64/Game-Win64-Shipping.exe \
  --input dlss --wine /path/to/runtime --engine vkd3d --graphics-api dx12 --dry-run
```

Inspect the preview, then repeat without `--dry-run`. That stages files only.
The renderer path is relative to the launch EXE's directory, or an absolute
path inside that directory. Symlinks cannot redirect installation outside it.
Directly launched renderers can omit `--renderer`. The binding also recognizes
the renderer's canonical path on a later direct launch.

Input values are `auto`, `dlss`, `fsr2`, `fsr3`, `fsr31` and `xess`. `fsr3` means
the older embedded FSR3 interface, while `fsr31` selects the FidelityFX API.
Auto accepts a known
recipe or one detected input; it rejects ambiguity. Select that input and its
quality preset inside the game. OptiScaler redirects the input to your chosen
output. Nucleon does not add temporal inputs to an arbitrary game.

Some game-specific recipes require the pinned OptiPatcher plugin to expose their
DLSS input. If preparation reports that this dependency is required, install it:

```sh
nucleon upscaler install patcher
```

The rolling release is checksum-pinned and will fail if replaced upstream.
`--archive /path/OptiPatcher.asi` can reuse the matching binary. The plugin is
enabled only by a matching recipe; it does not universally unlock DLSS.
The initial recipes keep frame generation, GPU spoofing and the overlay off.

## Launch selection and saved configuration

These commands **start the specified game**, and are separate from preparation:

```sh
nucleon run /games/Game/Game.exe --wine /path/to/runtime --upscaler fsr3.1
nucleon launch 12345 --upscaler fsr4-native-lite
nucleon bottle configure my-game --upscaler fsr2
nucleon bottle add my-game game /games/Game/Game.exe --upscaler fsr3.1
nucleon bottle run my-game game --upscaler game
```

A bottle preference applies only to applications with prepared bindings. An
installer or another EXE is not automatically injected. Standalone precedence
is one-launch CLI, saved application, bottle, then `game`. Steam uses its
one-shot launch override, `NUCLEON_UPSCALER`, per-game profile, then `game`.
Standalone launches ignore inherited Steam scaler preferences.

Profiles and bottle settings share this strict JSON schema:

```json
{
  "upscaler": {
    "backend": "fsr4-native-lite",
    "input": "dlss",
    "renderer": "Project/Binaries/Win64/Game-Win64-Shipping.exe"
  }
}
```

The whole object is one inherited preference. An absent setting inherits;
`{"backend":"game"}` explicitly selects the game's original behavior.
`bottle configure --clear upscaler` removes the bottle preference. A previously
prepared binding supplies its known input and renderer when a CLI selection
specifies only the output.

Selecting `game` reconciles/removes Nucleon's owned proxies and plugins before
launch. Choosing another scaler checks/stages the matching provider, forwarding
to the selected graphics backend. Explicit contradictory raw DLL preferences
are rejected; Steam and unrelated graphics preferences are retained. No scaler
choice silently changes the Wine runtime or graphics engine.

## Supported preparation routes

| Selected renderer | Managed choices |
| --- | --- |
| Intel GPTK D3DMetal, DX12 | Stock FSR2/3.1, experimental XeSS, MetalFX, native FSR4/Lite |
| Intel GPTK D3DMetal, DX11 | Stock FSR2/3.1 candidates |
| Wine + DXMT/DXVK, DX11 | Stock FSR2/3.1 candidates |
| Wine/Isotope + x64 vkd3d, DX12 | Stock FSR2/3.1; experimental XeSS |
| Isotope + x64 DXVK, DX11 | Stock FSR2/3.1 candidates |
| ARM64EC graphics, native Vulkan, x86 games, WineD3D/older APIs | Gated; retain `game` |

These are resolver gates, not a claim that all games work. Unreal XeSS input is
rejected because its depth contract is not established. MetalFX/native FSR4
requires DLSS input and the selected runtime's matching Intel GPTK components.
It works with `--engine wine` for complete vendor bundles such as Heroic's GPTK
4.0 beta 3, as well as a compatible managed GPTK engine. Native FSR4 currently
has no Mesa/vkd3d provider. MetalFX is not NVIDIA DLSS reconstruction.

## Inspect, switch and restore

```sh
nucleon upscaler status /games/Game/Launcher.exe
nucleon upscaler restore /games/Game/Launcher.exe
```

Ownership records, immutable provider generations, private panel settings and
logs live under `~/Library/Application Support/nucleon/upscalers/`. The same
renderer directory has one lock shared across prefixes, held for the game's
lifetime by Nucleon. Preparation/removal also checks for active Wine processes;
quit external launchers' game processes before changing their installation.

Existing unowned DLLs/INI files are conflicts. Nucleon does not overwrite an
unowned ReShade/OptiScaler install or construct an untested proxy chain. Modified
managed executable files and missing backups block all restore writes. An edited
managed INI remains in use across unchanged launches; before a provider switch
or restore, its contents are preserved under the binding's `user-settings/`.
Original permissions and symlinks are restored when a migrated record owns them.
An interrupted transaction can be recovered with `restore`.

Run receipts distinguish the requested/prepared provider from activation.
For new native builds, private status reports can establish active frame
replacement or MetalFX fallback with its reason and actual model/profile.
`status` labels these as the **last native report**, not proof a game is currently
running. Reports include encode/replacement counts, render/output dimensions,
model, preset, variant and the report's Unix timestamp when supplied by the hook.
`submission_interval_ms` is the hook's average CPU submission interval; it is
not scaler GPU cost or measured presentation FPS. Missing or invalid optional
measurements appear as `null`. Stock provider activation remains `not-observed`; its log and a
separate game/image test are needed. A selectable DLSS menu item is insufficient.

## Game compatibility and store launchers

Compatibility is determined by the game's rendering API, upscaler input and
selected runtime, rather than a blanket list of supported titles. The routes
above describe what Nucleon accepts for preparation; each game still needs an
in-game image and activation check. A game without a compatible temporal input
cannot gain one merely by selecting an output backend.

Keep the game's working prefix/bottle and store authentication route when
launching with upscaling. Saved applications can supply store arguments through
the configured authentication provider; see [saved launch profiles](STANDALONE.md#saved-native-launch-profiles).
Keep live authentication arguments out of saved profiles and shared logs.

See the [quick setup guide](tools/upscalers/README.md) and
[provider catalog](assets/upscalers/dependencies.toml) for dependencies and sources.

## MetalFX frame generation

`--frame-generation metalfx` enables the game's native DLSS-G path through the
prepared MetalFX bridge. `--frame-generation off` disables it. This is separate
from OptiScaler's FSR/XeSS frame-generation outputs, which remain disabled.
Currently the recipe is validated only for **Talos 2, DX12, Intel GPTK**, with
`--upscaler metalfx`. Other games and upscalers are rejected before launching.

```sh
nucleon app run talos2 --upscaler metalfx --frame-generation metalfx
nucleon app run talos2 --frame-generation off
```

Save `"frame_generation": "metalfx"` (or `"off"`) in a standalone profile's
`settings`, a bottle's settings, a bottle application entry, or a Steam game
profile. CLI choices override saved choices; `off` overrides an inherited enabled
setting. A saved enabled choice must also select the MetalFX upscaler. Omission
leaves the game's existing configuration alone; it does not undo a prior choice.

At launch, under the prefix lock, Nucleon updates Talos's existing Windows-user
settings: `r.Streamline.InitializePlugin`, `r.Streamline.DLSSG.Enable`, and
`FrameGeneration`. Start the game once before using this recipe. Existing settings
for older Wine usernames in that prefix are updated too, so switching the runtime
username does not silently lose the choice. Original INIs are backed up once as
`*.ini.nucleon-framegen.bak`; unrelated options are retained. Updates persist, and
an explicit `off` launch applies the inverse settings. Dry runs list the setting
and affected configuration directories without editing them.

The launch receipt records the requested choice and configuration preparation,
with activation initially `not-observed`. It does not infer successful generated
presentation from the setting alone. The tested bridge returned successful FG
evaluations with about two HUD presentations per evaluation; visual quality,
latency and long-session stability remain experimental.
