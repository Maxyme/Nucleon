# Upscaler providers

Nucleon manages optional upscalers for eligible x64 DX11/DX12 games. A game must
already expose a supported DLSS, FSR2/3/3.1 or XeSS input. Compatibility depends
on the game, runtime and graphics backend; see [supported routes](../../UPSCALING.md).

Install and prepare a stock provider for a DX12 game with DLSS input:

```sh
nucleon upscaler install stock
nucleon upscaler prepare /games/Game/Game.exe --backend fsr3.1 --input dlss \
  --wine /path/to/runtime --engine vkd3d --graphics-api dx12 --dry-run
```

Quit the game, inspect the preview, then repeat without `--dry-run` to prepare
its files. Use the actual rendering EXE, or supply `--renderer` for a separate
launcher. Launch with the same runtime and graphics choices, and enable the
chosen input in the game's settings:

```sh
nucleon run /games/Game/Game.exe --wine /path/to/runtime --engine vkd3d \
  --graphics-api dx12 --upscaler fsr3.1
nucleon upscaler status /games/Game/Game.exe
```

After quitting, `nucleon upscaler restore /games/Game/Game.exe` restores owned
files. Saved applications, bottles and Steam can use the same provider choices.

## MetalFX and native FSR4

These routes additionally require DX12, DLSS input and a complete matching Intel
GPTK runtime. Install the MetalFX provider and the forwarding bridge first:

```sh
nucleon upscaler install metalfx
nucleon upscaler install bridge --wine /path/to/complete/gptk/wine
```

The bridge requires `x86_64-w64-mingw32-gcc`. To build the optional native FSR4
provider, also run:

```sh
nucleon upscaler install native --wine /path/to/complete/gptk/wine
```

For the [upstream FSR 4.1.2 INT8 Lite model](https://github.com/UzenUPoZiTiV4ik/fsr4-native-macos),
use the separate provider and selection:

```sh
nucleon upscaler install native412 --wine /path/to/complete/gptk/wine
nucleon upscaler prepare /games/Game/Game.exe --backend fsr4.1.2 --input dlss \
  --wine /path/to/complete/gptk/wine --engine wine --graphics-api dx12 --dry-run
```

Quit the game and repeat without `--dry-run`, then launch with the same choices
and `--upscaler fsr4.1.2`. Enable DLSS in the game. The installer imports model
data from the checksum-pinned upstream v2.2 ZIP; `--archive /path/to/zip` reuses
that exact release. The existing `native` provider and its 4.1.1 choices stay available.

Native requirements are Python 3.11+ for upstream FSR4's build and shader tools,
Rosetta, Xcode with its Metal Toolchain, and `dxc`, `spirv-cross` and `spirv-val`
on PATH. `xcrun metal --version` must succeed. `--source /path/to/source` reuses a
local source tree; every required file is checked against the pinned source lock.

Rust owns downloads, source verification, compatibility patches, compiler setup,
build sequencing and validation reports. Original upstream source files remain
intact; generated overlays adapt depth handling and keep each game's panel state
private. The build validates synthetic Lite and full-quality Metal frames and
checks that the original GPTK framework remains unchanged. It never launches Wine
or a game. Gameplay activation must be checked separately through game-local
status and native frame reports.

Provider installation stages and validates a generation before atomic
publication. Failed builds preserve the previous generation, and retries can
repair incomplete installations. Existing provider identities remain compatible.

Pinned inputs and notices:

- [Provider releases and upstream revision](../../assets/upscalers/dependencies.toml)
- Reviewed source-file hashes: [4.1.1](../../assets/upscalers/fsr4-source.json), [4.1.2](../../assets/upscalers/fsr412-source.toml)
- [Third-party notices](../../NOTICE.md)
- [Complete upscaler guide](../../UPSCALING.md)
