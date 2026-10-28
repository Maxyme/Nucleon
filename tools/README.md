# Nucleon integration diagnostics

Wine/FEX builds, runtime assembly, CPU/GPU probes and source-inclusive publishing
have moved to the standalone sibling [isotope repository](../../isotope/README.md).
Its `isotope` executable has no Cargo dependency on Nucleon. Production
Nucleon consumes release archives through [wine install](../SETUP.md#install-an-isotopefex-runtime).

The remaining Rust `nucleon-devtools` crate contains Nucleon-specific diagnostics:

```sh
cargo build --release -p nucleon-devtools -p nucleon-cli

target/release/nucleon-devtools launch dirt --runtime /path/to/runtime --timeout 120
target/release/nucleon-devtools launch dmc5 --runtime /path/to/runtime --api dx12 --timeout 120

target/release/nucleon-devtools probe-steam --runtime /path/to/runtime \
  --compiler /path/to/x86_64-w64-mingw32-gcc
```

Game diagnostics require native Steam already running, the installed game and
local bridge. Game directories come from Nucleon's installation records;
`--game-dir` explicitly selects a different directory. They use the normal runner
with private profiles and isolated compatdata; without `--runtime` they use the
selected FEX default. `--game-dir`,
`--bridge`, `--cli`, `--work` and `--debug` override diagnostic inputs. DMC5
settings are locked, backed up and restored byte-for-byte even on failure or
interruption. Saves remain the real user's saves. These commands do not build or
implicitly assemble runtimes.

The Steam-route probe accepts an existing runtime and explicitly chosen compiler,
uses Nucleon's prefix/bridge preparation, and keeps its negative controls.
Supply `NUCLEON_STEAM_API` and optionally `NUCLEON_STEAM_BRIDGE`. It does not run a
game. Runtime build/graphics tooling no longer depends on installed games.

See the standalone builder for runtime validation and release CI.

## Standalone DX11 window check

`runtime-probes/d3d11-present.c` is a bounded x64 Windows test program: hardware
feature level 11.0, a real window/swap chain, twelve alternating-color frames,
full pixel readback and presentation. It needs neither Steam nor a game.

```sh
x86_64-w64-mingw32-gcc -O2 -Wall -Wextra -Werror \
  tools/runtime-probes/d3d11-present.c -o /tmp/nucleon-d3d11-present.exe \
  -ld3d11 -ldxgi -ldxguid -luser32
target/release/nucleon run /tmp/nucleon-d3d11-present.exe \
  --wine /path/to/compatible/intel-wine --emulator rosetta \
  --engine dxmt --graphics-api dx11 --upscaler game
```

Success requires the `NUCLEON_D3D11_OK` marker and exit 0. See
[Wine setup](../SETUP.md#download-and-patch-the-recommended-build) for the
reviewed Wine 11.18 patches and their limits. A successful probe does not establish gameplay.
