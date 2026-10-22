# Talos 2: native FSR4 Lite preparation

This experimental integration stages **FSR 4.1.1 Lite (`lossy`)** behind Talos's
DLSS inputs. It combines OptiPatcher, a pinned OptiScaler MetalFX passthrough
fork, GPTK's own `nvngx-on-metalfx.dll`, and the native macOS FSR4 hook.
Both the stock FSR 3.1 baseline and native FSR 4.1.1 Lite passed gameplay tests.
For Lite, the user confirmed the DLSS option, responsive menus and working
movement; native status recorded 5,358 replaced frames and a clean exit.
See the [current Talos results](../../TALOS2-UPSCALING.md).

Nucleon's shared [upscaler manager](../../UPSCALING.md) now provides typed launch
choices and game-local ownership. Existing Talos preparations remain with this
helper until `nucleon upscaler migrate-talos` transfers the journal. The store
wrapper then forwards the generic owner's choice; restoration uses the manager.

`prepare` builds and runs **synthetic native Metal tests only**. It never runs
Wine, Legendary or Talos, changes registry settings, or edits Heroic preferences.
Only the explicit `launch` and internal `wrapper` commands can start the game.

## Prepare and inspect

Requirements: Python 3.11+, Xcode with its Metal Toolchain component, `dxc`,
`spirv-cross`, `spirv-val`, Rosetta, a built Nucleon CLI and a working complete
Intel GPTK bundle. `xcrun metal --version` must succeed. If necessary, install the
official compiler component using `xcodebuild -downloadComponent MetalToolchain`.

From the Nucleon repository:

```sh
python3 tools/upscalers/talos2.py prepare
python3 tools/upscalers/talos2.py status
python3 tools/upscalers/talos2.py launch --dry-run
```

The defaults select the installed Heroic Talos 2 game/prefix and GPTK 4.0 beta 3.
`prepare --help` lists overrides. Downloaded dependencies are pinned in
`dependencies.json`; assets are checked against their release SHA-256 digests,
and FSR source files against the pinned Git blob identities. A changed rolling
OptiPatcher download fails its checksum rather than silently installing an update.
`--fsr-source`, `--optiscaler` and `--optipatcher` can reuse verified local inputs.

Private sources, shaders, the copied framework, logs and rollback records live in:

```text
~/Library/Application Support/nucleon/upscalers/talos2-fsr4-lite/
```

No original runtime or game executable is patched. Setup stages these files
beside the **renderer**, under `Talos2/Binaries/Win64/`:

```text
dxgi.dll                                      OptiScaler proxy
OptiScaler.ini                                MetalFX passthrough, FG/spoofing/overlay off
OptiScaler/plugins/OptiPatcher.asi              Talos DLSS availability patch
nvngx.dll                                    Original Wine builtin PE loader entry
nvngx-on-metalfx.dll                          Selected GPTK's original-named bridge
OptiScaler/nucleon-metalfx/nvngx.dll            Complete NGX forwarding DLL
```

The overlay is disabled to avoid unnecessary D3D11 overlay hooks during this
first DX12 test. The native FSR panel remains enabled. Its private configuration
starts in Lite mode, and its log can distinguish an active replacement from a
fallback to regular MetalFX. Selecting FSR4 in stock OptiScaler is not this path.

## macOS 27.2 compatibility adaptation

The synthetic live hook test exposed a missing `isDepthReversed` getter on
`_MFXTemporalScalingEffectBBR`. `fsr4_depth_compat.h` captures the real
`setDepthReversed:` value, using the public getter when present. An unavailable
value falls back to MetalFX. Subclass hooks preserve the original superclass
setter, including either hook installation order. A generated source overlay
uses this adaptation; pinned upstream source files remain unchanged.

Setup tests the accessor shim, loads the copied framework, computes synthetic
FSR4 Lite frames through the native INT8 backend, and compares the live hook's
output with the standalone reference. Original framework hashes are checked
before and after the build. Results are recorded in
`fsr4/dist/crossover/nucleon-validation.json` and `profile.json`.

## Later game test — explicit separate step

When ready to start the game:

```sh
python3 tools/upscalers/talos2.py launch
```

This uses Heroic's existing Legendary account configuration to obtain fresh Epic
authentication and wraps the game with Nucleon, Rosetta and the same GPTK bundle.
No authentication arguments are saved in the profile. Legendary output stays in
a private log because it can include credentials. Avoid sharing raw game logs.

In the game, first confirm DLSS is selectable, select DLSS Quality, and verify
the FSR panel reports **FSR 4.1.1 active**. An open game window or an enabled
menu item alone does not establish that FSR4 Lite is processing frames.

## Restore

Quit the game and its prefix, then run:

```sh
python3 tools/upscalers/talos2.py restore
```

Original files, permissions and symlinks are backed up before deployment.
Removal checks every installed file and backup before changing anything. It
refuses to overwrite later user/game edits, including a changed OptiScaler INI;
preserve those changes before restoring. An interrupted deployment has a journal
and can be restored. Another active prefix or Nucleon session blocks setup/removal.
The private build and backups remain available after removal.

## Validation without a game

```sh
python3 -m unittest discover -s tools/upscalers -p 'test_*.py' -v
clang -fobjc-arc -arch x86_64 -Wall -Wextra -Werror -framework Foundation \
  tools/upscalers/test_depth_compat.m -o /tmp/nucleon-depth-compat
arch -x86_64 /tmp/nucleon-depth-compat
```

Sources and licenses:

- FSR4: https://github.com/UzenUPoZiTiV4ik/fsr4-native-macos (MIT source and retained third-party notices)
- MetalFX OptiScaler fork: https://github.com/vm99cs8fnw/OptiScaler-witcher3-skipspoof
- OptiScaler: https://github.com/optiscaler/OptiScaler (GPL-3.0)
- OptiPatcher: https://github.com/optiscaler/OptiPatcher

The fork was demonstrated upstream with Witcher 3. Talos now initializes this
bridge and processes native Lite frames; the separate gameplay and performance
validation status is linked above.
