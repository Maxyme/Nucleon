# Nucleon

Nucleon allows running Windows games from the native macOS Steam client using a combination of runtimes (Wine, GPTK4) and different graphical backends.  

The goal is to launch Steam games with minimal configuration, automatic 32/64-bit
handling, and reproducible per-game runtimes. 
Compatibility depends on the game, Wine build, backend, and driver; support is experimental.

Note: **CrossOver** is optional but can be used like other wine runtimes.

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

You need Apple Silicon macOS, native Steam, a current Rust toolchain supporting
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

## Development

```sh
just fmt       # Format source
just lint      # Formatting and Clippy checks
just test      # Run tests when wanted
```

Keep confirmed profiles pinned. Change one component at a time and record
visual/input checks before promoting a new configuration.

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
