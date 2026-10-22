# Nucleon

Nucleon runs Windows games and programs on Apple Silicon macOS, directly or from
the native Steam client, using Wine (including Isotope/FEX), GPTK and different
graphics backends.

The goal is to launch Steam games with minimal configuration, automatic 32/64-bit
handling, and reproducible per-game runtimes. 
Compatibility depends on the game, Wine build, backend, and driver; support is experimental.

Note: **CrossOver** is optional but can be used like other wine runtimes.

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

Without `--bottle`, each canonical EXE path gets its own environment. Bottles keep
their selected runtime, preferences, installed files and saves across launches.
See [standalone usage](STANDALONE.md) for saved applications, configuration and
runtime selection.

## Running locally

From the checkout, build and use the local CLI:

```sh
just build
./target/release/nucleon setup
./target/release/nucleon status
```

Quit Steam before setup, restart it afterward, and launch the game through its
Nucleon compatibility tool. See [SETUP.md](SETUP.md) for prerequisites, Wine and
backend configuration, and bridge staging.

## Build and setup

For Steam integration, you need Apple Silicon macOS, native Steam, a current Rust toolchain supporting
edition 2024, Xcode Command Line Tools, and `just`. Runtime requirements vary;
obtain Wine, graphics backends, and bridge components under their own licenses.

Previous releases have been withdrawn. Build from source:

```sh
git clone https://github.com/Maxyme/Nucleon.git
cd Nucleon
rustup target add aarch64-apple-darwin x86_64-apple-darwin
brew install just
just build
./target/release/nucleon wine list
./target/release/nucleon setup
```

Read setup's results: missing bridge files or failed assembly need attention.
Quit Steam before applying integration changes and restart afterward.
The [setup guide](SETUP.md) covers Wine, GPTK, backends, bridge staging, and removal.

In Steam, open a game's **Properties → Compatibility**, enable the tool override,
and select a Nucleon tool. Existing native macOS games should keep their native
configuration. Use **Play** normally after configuration.

Prebuilt ARM64 Wine/FEX core runtime prereleases are built in the separate
`isotope` repository. See [runtime releases](RUNTIME-RELEASES.md) for publisher automation,
source/license contents, installation commands and the current graphics limits.

Install an exact Wine/FEX version or a local runtime:

```sh
nucleon wine install isotope --version 0.1.0-rc.1 --register
nucleon wine install /path/to/runtime-or-archive --register
```

The named command requires that release to be published; local paths work before
publication. Select a complete installed Wine runtime per launch:

```sh
nucleon launch 690790 --wine fex-preview --engine dxmt
nucleon launch 690790 --emulator fex --engine dxmt
nucleon launch 690790 --emulator rosetta --engine dxvk
```

`--wine` accepts an exact installed ID or runtime path. `--emulator rosetta`
uses the ordinary configured Intel Wine; `--emulator fex` uses the runtime
registered with `nucleon wine install ... --register` or `wine fex register`.
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
redistribution requirements. [NOTICE.md](NOTICE.md) records attribution,
historical permissions, and unresolved provenance questions. The change does
not revoke earlier grants or authorize redistribution of Valve or Apple components.
