# Nucleon integration diagnostics

Wine/FEX builds, runtime assembly, CPU/GPU probes and source-inclusive publishing
have moved to the standalone sibling [isotope repository](../../isotope/README.md).
Its `isotope` executable has no Cargo dependency on Nucleon. Production
Nucleon consumes release archives through [wine install](../RUNTIME-RELEASES.md).

The remaining Rust `nucleon-devtools` crate contains Nucleon-specific diagnostics:

```sh
cargo build --release -p nucleon-devtools -p nucleon-cli -p nucleon-runner

target/release/nucleon-devtools launch dirt --runtime /path/to/runtime --timeout 120
target/release/nucleon-devtools launch dmc5 --runtime /path/to/runtime --api dx12 --timeout 120

target/release/nucleon-devtools probe-steam --runtime /path/to/runtime \
  --compiler /path/to/x86_64-w64-mingw32-gcc
```

Game diagnostics require native Steam already running, the installed game and
local bridge. They use the normal runner with private profiles and isolated
compatdata; without `--runtime` they use the registered FEX default. `--game-dir`,
`--bridge`, `--runner`, `--work` and `--debug` override diagnostic inputs. DMC5
settings are locked, backed up and restored byte-for-byte even on failure or
interruption. Saves remain the real user's saves. These commands do not build or
implicitly assemble runtimes.

The Steam-route probe accepts an existing runtime and explicitly chosen compiler,
uses Nucleon's prefix/bridge preparation, and keeps its negative controls.
Supply `NUCLEON_STEAM_API` and optionally `NUCLEON_STEAM_BRIDGE`. It does not run a
game. Runtime build/graphics tooling no longer depends on installed games.

See the standalone builder for runtime validation and release CI.
