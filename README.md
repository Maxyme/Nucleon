# Nucleon

Nucleon runs Windows games and programs on Apple Silicon macOS using Wine
(including Isotope/FEX), GPTK and other graphics backends. It launches Steam games
itself while the ordinary Steam client supplies ownership and Steamworks services.

Steam stays unmodified. Nucleon uses explicit installation records and per-game
runtimes.
Game compatibility remains experimental and depends on the runtime and graphics
backend. CrossOver is optional.

## Steam games

Install [DepotDownloader](https://github.com/SteamRE/DepotDownloader), then use:

```sh
nucleon steam install 1622910 --username <account>
nucleon steam show 1622910
nucleon launch 1622910
nucleon steam update 1622910 --username <account>
```

The downloader requests your password or Steam Guard approval interactively;
Nucleon does not accept or store a Steam password. Downloads explicitly select
Windows files and live under Nucleon's support directory. Updates are requested
through Nucleon, rather than desktop Steam's Downloads screen. Set `--downloader`
when the provider is outside PATH; an installation under
`<support>/tools/depotdownloader/DepotDownloader` is discovered automatically.

Keep Steam running and signed in when playing. Use `steam show` and
`--launch-id <id>` for games with several launch choices, or `--exe <path>` to
select an executable inside the Nucleon installation. Game arguments follow `--`.
`launch --dry-run` previews the selected runtime and prefix without starting a
game. Per-game runtime profiles configure Wine, graphics and Steamworks settings.

## Standalone EXEs and named environments

Build the CLI with `cargo build --release -p nucleon-cli`. Standalone launches
need a complete Wine runtime; Steam setup and bridge files are unnecessary.

```sh
nucleon run /path/Game.exe --wine /path/to/runtime --engine auto -- --fullscreen

# Share one environment between an installer and its installed game.
nucleon bottle create my-game --wine /path/to/runtime
nucleon run /path/setup.exe --bottle my-game
nucleon bottle show my-game
nucleon bottle add my-game game /path/to/installed/Game.exe
nucleon bottle run my-game game
```

Add `--dry-run` to `nucleon run` or `nucleon app run` to inspect the selected
runtime, engine, graphics components and prefix. Preview uses the same resolution
as execution, without authenticating, starting Wine or changing a prefix.

Without `--bottle`, each canonical EXE path gets its own environment. Bottles keep
their selected runtime, preferences, installed files and saves across launches.
See [standalone usage](STANDALONE.md) for saved applications, configuration and
runtime selection.

## Upscaling games

Nucleon can use OptiScaler to replace a game's existing temporal upscaler with
FSR2, FSR 3.1, experimental XeSS, MetalFX or native FSR 4.1.1/Lite and 4.1.2 INT8 Lite. It works
with direct EXEs, saved applications, bottles and Steam.

Eligible games are **x64 DX11/DX12 games with compatible DLSS, FSR2/3/3.1 or
XeSS inputs**. Support depends on the game and runtime; meeting these requirements
does not guarantee compatibility. MetalFX/native FSR4 additionally requires
**DX12, DLSS input and a complete matching Intel GPTK runtime**. Managed upscaling
currently excludes x86 games, native Vulkan, ARM64EC graphics and plain WineD3D.
Unreal XeSS input is also excluded; XeSS output is experimental and DX12-only.

For a DX12 game exposing DLSS, using an already working vkd3d runtime:

```sh
nucleon upscaler install stock
nucleon upscaler prepare /games/Game/Game.exe --backend fsr3.1 --input dlss \
  --wine /path/to/runtime --engine vkd3d --graphics-api dx12
nucleon run /games/Game/Game.exe --wine /path/to/runtime --engine vkd3d \
  --graphics-api dx12 --upscaler fsr3.1
```

Quit the game before preparation; add `--dry-run` to `prepare` to preview its
changes first. Use the game's actual rendering EXE, or specify `--renderer` for
a separate launcher. Keep the runtime, engine and prefix/bottle consistent with
your normal launch. In the game, enable the selected **input** (DLSS in this
example) and choose its quality preset; OptiScaler supplies the requested output.

Use `nucleon upscaler status /games/Game/Game.exe` to inspect the binding, or
`nucleon upscaler restore /games/Game/Game.exe` after quitting to restore owned
files. See the [quick setup guide](tools/upscalers/README.md) for GPTK/native FSR4
and [upscaler reference](UPSCALING.md) for compatibility, saved preferences and
activation checks. Preparation alone does not establish in-game activation.

## Build and setup

Use a current Rust toolchain supporting edition 2024 and Xcode Command Line Tools.
Build from source:

```sh
cargo build --release -p nucleon-cli
./target/release/nucleon setup
./target/release/nucleon status
```

`setup fix` prepares Nucleon's directories, runtime and supplied Steamworks bridge.
It does not modify Steam or install a service. Steam games need matching client
libraries and a bridge for the selected Wine architecture. Standalone applications
do not require those files. Read the [setup guide](SETUP.md) for details.

Prebuilt ARM64 Wine/FEX core runtime prereleases are built in the separate
`isotope` repository. See [runtime releases](RUNTIME-RELEASES.md) for publisher automation,
source/license contents, installation commands and the current graphics limits.

Install an exact Wine/FEX version or a local runtime:

```sh
nucleon wine install isotope --version 0.1.0-rc.1
nucleon wine install /path/to/runtime-or-archive
```

The named command requires that release to be published; local paths work before
publication. Select a complete installed Wine runtime per launch:

```sh
nucleon launch 690790 --wine /path/to/fex-runtime --engine dxmt
nucleon launch 690790 --emulator fex --engine dxmt
nucleon launch 690790 --emulator rosetta --engine dxvk
```

`--wine` accepts an exact installed ID or runtime path. `--emulator rosetta`
uses the ordinary configured Intel Wine; `--emulator fex` uses the runtime
selected with `nucleon wine fex select <runtime>`.
Combining them constrains the selected runtime's mode. These options select
complete compatible Wine builds, and do not change either default for one launch.
Run `nucleon wine list` to see available IDs. The FEX core preview omits the developer graphics stack. Its 32-bit execution
test currently has a known child-process failure, recorded as non-blocking
in the separate builder.

Runtime builds and publishing live in [isotope](../isotope/README.md).
Nucleon-specific diagnostics use [`nucleon-devtools`](tools/README.md).

## Development

```sh
just fmt       # Format source
just lint      # Formatting and Clippy checks
just test      # Run tests when wanted
```

Keep confirmed profiles pinned. Change one component at a time and record
visual/input checks before promoting a new configuration.

Graphics backend release sources are defined in [assets/downloads.toml](assets/downloads.toml):
default versions and URL templates. Archive formats are inferred from URL suffixes. `{tag}` includes the `v` prefix;
`{version}` excludes it. The catalog is embedded at build time; rebuild after editing it.

## Similar projects / Inspiration

These are projects that share a similar goal, and were used as inspiration for this project. 

- https://github.com/NotProtonNot/NotProton
- https://github.com/fxgl/steamac
- https://github.com/Broyojo/hadron
- https://github.com/dttdrv/mage

## License

[GPL-3.0-only](LICENSE). Third-party runtimes retain their own licenses and
redistribution requirements. [NOTICE.md](NOTICE.md) records third-party
attribution and component notices.
