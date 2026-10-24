# Standalone Windows programs and bottles

Build the CLI with `cargo build --release -p nucleon-cli`, then use
`target/release/nucleon` (or put that binary on your PATH). No Steam installation,
Steam patching or bridge files are required for the commands on this page.

## Run an EXE

```sh
nucleon run /path/Game.exe --wine /path/to/runtime
nucleon run ./Game.exe --emulator fex --engine dxvk -- --fullscreen "argument with spaces"
nucleon run ./Tool.exe --wine /path/to/runtime --engine wine --cwd /path/to/data
```

Nucleon accepts x86 and x64 Windows executables and checks that the selected Wine
supports the target. Paths resolve from the directory where you invoke Nucleon;
the program's working directory defaults to its EXE's parent. `--cwd` changes that
directory. Arguments following `--` go to the Windows program without shell
interpretation, including arguments that resemble Nucleon options.

ARM64/ARM64EC application targets are rejected. A runtime's matched ARM64EC
graphics frontends can still be used by ordinary x64 applications.

Choose a complete Wine installation using `--wine <exact-id-or-path>`, or use
`--emulator fex|rosetta` to select its configured default. Combining the options
requires that Wine to use the requested emulator. To select FEX without creating
a Steam compatibility tool:

```sh
nucleon wine fex select /path/to/installed/isotope-runtime
nucleon run /path/Game.exe --emulator fex
```

With no runtime option, standalone launches use the selected FEX default if one
exists, otherwise the configured Intel/Rosetta Wine. Existing FEX Steam tool
selections remain usable as a fallback until an independent selection is saved.
A broken saved selection reports an error instead of switching runtimes.

Graphics options are `--engine auto|gptk|dxmt|dxvk|vkd3d|kosmickrisp|wine` and
`--graphics-api auto|dx7|dx9|dx10|dx11|dx12|vulkan|opengl`. Availability depends on
the selected runtime and installed compatible graphics components. Isotope's
core package supplies WineD3D; a Vulkan engine additionally needs a compatible
graphics stack. GPTK requires a complete compatible Intel Wine runtime bundling
its matched D3DMetal components, selected explicitly with `--wine`; it does not
replace a selected Wine runtime with another runner.
`--hud` enables the Metal HUD; `--hud=false` explicitly disables it.

`--upscaler game|fsr2|fsr3.1|xess|metalfx|fsr4-native-lite|fsr4-native`
selects an experimental prepared upscaler. Install the provider and bind the
rendering EXE before launch; bottle defaults do not automatically inject
installers. See [upscaling](UPSCALING.md) for preparation, saved application
preferences, supported graphics paths and restoration.

Each canonical EXE path gets a separate data directory by default. A symlink to
that same EXE uses the same identity. Moving or renaming an EXE selects a new
identity; use a bottle for a stable environment shared by multiple executables.

Steam AppIDs, compatibility data paths, Steam profiles, tool pins and one-shot
launch files inherited from the parent are ignored. Select standalone Wine and
engine settings through these command options or bottle settings rather than
`NUCLEON_WINE`/`NUCLEON_ENGINE`. Existing diagnostic controls such as
`NUCLEON_DEBUG=1` and `NUCLEON_RUN_SECONDS` remain available.

EXE installers can run like other programs. MSI/shortcut dispatch and prerequisite
installation are separate features. A Steam-dependent game can still require its
normal Steam launch and authentication.

To reuse an initialized Heroic or other Intel prefix in place, use
`--prefix /path/to/prefix --emulator rosetta`. This is separate from `--bottle`.
The prefix must already contain `drive_c`, `system.reg` and `user.reg`. Nucleon
locks it for the launch and stages the selected graphics components there.
FEX cannot adopt an existing Intel prefix.

For an existing Heroic GPTK bundle, `--engine wine` runs the bundle's own graphics
components and preserves its native library resolution. Select the bundle's
`Contents/Resources/wine` directory with `--wine`. Store authentication still
needs to come from the store launcher; Talos 2's Epic edition exits and requests
a store relaunch when started without it. Launching Legendary with Nucleon as a
wrapper preserves that authentication without saving it in a bottle.

`NUCLEON_WINEDLLOVERRIDES` appends explicit DLL preferences to the launch's
graphics overrides, for example `d3dcompiler_47=n,b`. Rosetta feature overrides
include `NUCLEON_ROSETTA_ADVERTISE_AVX_COPY`, `NUCLEON_D3DM_MTL4` and
`NUCLEON_D3DM_SUPPORT_DXR`. Intel prefix setup and execution use matching esync
and msync settings so a server started during setup can also host the game.

## Saved native launch profiles

Save an EXE's runtime, existing prefix, working directory, environment and
arguments, then launch it through Nucleon itself:

```sh
nucleon app add my-game "/Games/My Game/Game.exe" \
  --prefix "/Prefixes/My Game" --wine /path/to/wine \
  --emulator rosetta --engine dxmt -- --fullscreen
nucleon app run my-game --dry-run
nucleon app run my-game
```

`app run` accepts `--exe`, `--prefix`, `--cwd` and the normal runtime/graphics
options as one-launch overrides. Arguments after `--` append to the saved game
arguments. Paths and arguments are passed directly, without shell parsing.
`nucleon run /another/path.exe` remains available without saving a profile.

```sh
nucleon app list
nucleon app show my-game
nucleon app run my-game --exe "/Moved Game/Game.exe" -- --another-option
```

For Epic games requiring authentication, Nucleon requests fresh JSON launch
arguments from the installed Legendary tool, then runs the EXE through its own
native runner. It does not run the old Python wrapper or Legendary's configured
Wine/wrapper/pre-launch commands. The external Legendary binary remains required
for account authentication; Nucleon does not implement Epic login. Use an
existing signed-in Heroic/Legendary configuration:

```sh
nucleon app add epic-game "/Games/Epic Game/Game.exe" \
  --prefix "/Prefixes/Epic Game" --wine /path/to/wine --emulator rosetta \
  --legendary /path/to/legendary --legendary-app STORE_APP_ID \
  --legendary-config /path/to/legendary-config
nucleon app check epic-game
nucleon app run epic-game
```

`app check` validates the profile and requests authentication arguments but never
starts Wine or the game. `app run --dry-run` does not even request authentication.
Fresh account arguments remain in memory, are not saved in the profile and are
redacted from Nucleon's diagnostic argument traces. Imported store environment
values are applied before explicit profile environment preferences.

Use repeatable `app add --env NAME=value` for application-specific settings.
Prefix/runtime identity must use typed options, rather than `WINEPREFIX`,
`WINEARCH`, `NUCLEON_WINE`, `NUCLEON_ENGINE` or Steam identity variables.

Advanced profiles can be imported with `nucleon app import NAME launch.json`.
Schema 1 contains `application` (executable, arguments and cwd), `settings`
(the same fields as bottle settings), optional `prefix`, `environment` and
optional `authentication` (provider `legendary`, binary, app_id and config).
Relative file paths resolve against the JSON document; a relative upscaler
renderer resolves against the EXE directory, matching the upscaler manager.
Profiles are stored under Nucleon's support directory in `applications/`.
Existing entries require explicit `--replace` to overwrite.

The migrated local Talos profile launches with:

```sh
nucleon app run talos2
```

It preserves the existing Epic prefix, Rosetta/GPTK runtime and prepared native
FSR 4.1.1 Lite configuration. Importing it does not run the game. Upscaler
bindings are installation-specific; moving a modded game still requires its
normal provider preparation and validation at the new path.

## Create and configure a bottle

A bottle is a named environment with shared prefix data, graphics preferences
and saved application entries. Create it before using `--bottle`:

```sh
nucleon bottle create my-game --emulator fex --engine auto
nucleon run /path/setup.exe --bottle my-game
nucleon bottle list
nucleon bottle show my-game
nucleon bottle show my-game --json
```

Creation validates and pins the selected Wine generation but prepares the Windows
prefix only on first launch. `show` prints the bottle's data location, settings
and application entries. Installed programs and saves live in the prefix beneath
that data directory, unless a program deliberately stores them elsewhere.

```sh
nucleon bottle configure my-game --engine dxvk --graphics-api dx11 --hud
nucleon bottle configure my-game --clear graphics-api --clear hud
nucleon bottle configure my-game --reset
```

`configure` changes only supplied fields. `--clear` removes named preferences;
`--reset` clears all preferences, including the Wine pin, before applying supplied
options. These commands retain installed files, saves and saved application
entries. With the Wine pin cleared, subsequent launches resolve the current
default (or a retained emulator preference).

One-launch options override bottle preferences without saving changes:

```sh
nucleon run /path/Game.exe --bottle my-game --engine wine --hud=false
```

Both runtime and emulator preferences constrain selection. When changing to a
runtime using another emulator, update or clear a conflicting saved emulator:

```sh
nucleon bottle configure my-game --wine /path/to/other/wine --clear emulator
```

Intel and FEX prefixes are separate. FEX prefixes are further separated by runtime
family; an incompatible ABI change within a family is rejected. A new namespace
does not migrate saves from the previous prefix. Compatible updates reuse prefix
data, but changing Wine back does not undo registry or prefix changes made by a
newer version. Keep a backup before changing a working runtime.

## Save application entries

Installers and installed programs can have separate entries in one bottle:

```sh
nucleon bottle add my-game installer /path/setup.exe
nucleon bottle run my-game installer
nucleon bottle add my-game game "/path/to/installed/Game.exe" --cwd /path/to/game-data -- --fullscreen
nucleon bottle run my-game game
nucleon bottle run my-game game --engine dxvk -- --additional-option
```

Entries save the canonical executable path, arguments and optional working
directory. Extra arguments on `bottle run` append to saved arguments. Its runtime,
graphics, HUD and working-directory options apply to that launch only. Use the
data location from `bottle show` to locate a program installed inside the prefix;
EXE paths are native macOS filesystem paths.

Existing names require an explicit replacement:

```sh
nucleon bottle add my-game game /new/path/Game.exe --replace
nucleon bottle forget my-game installer
```

`forget` removes only the saved entry, leaving installed files and saves intact.
Configuration updates are atomic and serialized. A second launch using the same
prefix is rejected while the first owns it; no shared Wine server is killed by
that failed attempt. Installer child processes remain supervised, and shutdown
stops only the owned prefix/process groups.

## Logs and validation

`nucleon logs --runner` shows the latest launch. Standalone logs are retained
under `<support>/logs/standalone/<application-or-bottle-id>/<run-id>/`, including
`setup.log`, `wine.log` and `receipt.json`. Receipts identify standalone origin,
application/bottle identity, resolved Wine, engine, prefix and launch timing.
The primary process exit code is preserved; when a launcher hands off to children,
the children's eventual individual exit codes are not reported by the launcher.

The normal test suite covers CLI parsing, target validation, prefix bootstrap and
retry, Steam-independent environment construction, identity/persistence and
process classification. On Apple Silicon, the optional integration test compiles
an owned console program using MinGW and runs it in temporary prefixes:

```sh
NUCLEON_TEST_STANDALONE_RUNTIME=/path/to/isotope-runtime \
  cargo test --locked -p nucleon-cli --test standalone -- --ignored --nocapture
```

It requires `x86_64-w64-mingw32-gcc`, `i686-w64-mingw32-gcc`, a runtime supporting
both guest architectures, and macOS access for Wine's local IPC sockets.
It checks argument/working-directory handling, Steam-state isolation, separate
application prefixes, shared installer/game state, child handoff, locking and
cancellation with an unrelated-process control. A second probe checks x86 execution
with Unicode paths and arguments. These tests do not launch installed
games or establish game compatibility.
