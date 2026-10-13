# Setup

[README](README.md) · [Architecture](ARCHITECTURE.md) · [Troubleshooting](TROUBLESHOOTING.md)

## Build

Install a current Rust toolchain supporting edition 2024, Xcode Command Line
Tools, and `just`. Apple Silicon and Rosetta are the confirmed Wine host path;
requirements vary by runtime and backend.

```sh
xcode-select --install
rustup target add aarch64-apple-darwin x86_64-apple-darwin
brew install just
just build
```

Use `./target/release/nucleon` below, or install it on your PATH and use `nucleon`.
`just build` also prepares the universal Steam hook. Earlier release bundles
were withdrawn; build from source until another release is published.

## Register Wine

```sh
nucleon wine list
nucleon wine add my-wine /path/to/wine --use-now
nucleon wine use my-wine
```

Discovery is not validation. Confirmed recipes use Wine 11.18 with specific
patches, not an arbitrary newer version. Preserve a working build and profile.
CrossOver can be registered, but is not required.

## Optional GPTK

Obtain Apple's components under their terms. Supply `D3DMetal.framework` and
`libd3dshared.dylib` from your mounted toolkit; volume/package paths vary.

```sh
nucleon gptk set-path /path/to/gptk-components
nucleon gptk verify /path/to/gptk-components
nucleon gptk status
```

Verification checks ABI pairing with the selected Wine, not game rendering.
The former overlay's experimental Wine bridge is absent; GPTK configurations
need another gameplay check before deployment.

## Graphics configuration

```sh
nucleon backends status
nucleon dxvk set-path /path/to/dxvk
nucleon dxmt set-path /path/to/dxmt
nucleon vkd3d set-path /path/to/vkd3d-proton
nucleon d7vk set-path /path/to/d7vk
nucleon kosmickrisp set-path /path/to/driver-or-icd
```

These are alternatives, not a required sequence. Automatically fetched upstream
releases may differ from the macOS builds used in confirmed profiles. Vulkan
drivers must match Wine's host architecture and the backend's feature needs.

## Steam bridge and integration

You need Windows client libraries matching installed Steam and the bridge/helper
required by your recipe. The Valve staging manifest does not build a macOS
`lsteamclient` port. Preserve exact binary source revisions and license notices.

With the required client files extracted:

```sh
nucleon setup --bridge-path /path/to/extracted-client-files
nucleon status
```

Quit Steam before integration changes and restart afterward. Setup may report
partial remediation; inspect failures instead of assuming registration means a
game can run. See `nucleon setup --help` and `nucleon setup fix --help`.

In Properties → Compatibility, force a Nucleon tool for the Windows game. Leave
native macOS titles native. Confirm actual runtime/backend selection in logs.

## Profiles and launch options

Profiles: `~/Library/Application Support/nucleon/game_profiles/<AppID>.json`.
Example shape, with paths that must be replaced:

```json
{
  "engine": "auto",
  "wine": "/path/to/confirmed-wine",
  "dxvk_path": "/path/to/confirmed-dxvk",
  "vulkan_icd": "/path/to/confirmed-driver.json",
  "use_steam_shim": true,
  "overlay": false
}
```

Omit irrelevant backend fields. DMC5's recipe instead uses loader-patched Wine
and direct execution; do not copy the helper choice blindly. Isolate support
and prefix directories for experiments and save the old profile.

Steam launch options include:

```sh
NUCLEON_WINE=/path/to/wine %command%
NUCLEON_WINEDEBUG=+timestamp,+loaddll,+msgbox,+dialog %command%
NUCLEON_OVERLAY=1 %command%
```

Overlay injection defaults off. A profile's `overlay: false` wins over opt-in.
The removed shim's HDR/Metal 4 fixes and microphone guards are not supplied.

## Guard and removal

```sh
nucleon guard status
nucleon guard run
nucleon guard install
nucleon guard uninstall
nucleon unregister
```

Guard repairs local integration; new Steam builds still need hook validation.
Unregister removes integration/tool mappings. Preserve runtime/prefix backups
before cleanup and consult command help for exact behavior.
