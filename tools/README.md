# Wine/FEX developer tooling

The maintained build, packaging and diagnostic workflows are implemented by the
Rust `nucleon-devtools` crate. The eight Python implementation scripts and their
five test files have been retired; their historical versions remain in Git.
The supported Wine route is the pinned Tolkara ARM64/FEX fork. The former
MacRunner loader route, which did not pass Windows execution, remains historical
research rather than another supported build variant.

Build the tools from the checkout:

```sh
cargo build --release -p nucleon-devtools -p nucleon-cli -p nucleon-runner
```

End users install complete runtimes with `nucleon wine fex install`; they do not
need this developer CLI. Runtime/default selection and normal game launches stay
in `nucleon`. Builds and packaging do not select installed runtimes or launch
Steam/games.

## CPU runtime

On native Apple Silicon macOS, install Xcode Command Line Tools, current
Bison/Flex, CMake, Ninja and Python 3.9+. Our orchestration is Rust, but the pinned
FEX CMake project still requires a Python interpreter. Graphics builds also use
Meson and its pinned Python dependencies.

```sh
target/release/nucleon-devtools fex fetch --reference
target/release/nucleon-devtools fex doctor
target/release/nucleon-devtools fex build-fex
target/release/nucleon-devtools fex build-wine
target/release/nucleon-devtools fex probe --window
target/release/nucleon-devtools fex probe-unixlib
```

`--work` chooses the developer workspace (default `target/arm64-fex`); `--jobs`
controls compiler parallelism. Source and compiler archives are pinned and
verified. `fetch` owns downloads; builds/probes use existing inputs. The CPU
probe retains its shared-data negative control. The native companion probe checks
its function table, unsupported controls, PID and file-mapping/FD ownership
without loading Wine. `fex check-reference` verifies the cached reference archive
and launcher signature without executing that launcher.

## Graphics and complete developer generations

The DXMT baseline uses checksum-pinned reference graphics and is a local-import
experiment. It is not a redistribution package or an ARM64EC graphics default.

```sh
target/release/nucleon-devtools fex probe-dxmt
target/release/nucleon-devtools dx12 fetch --sdk "$HOME/VulkanSDK/1.4.363.0/macOS"
target/release/nucleon-devtools dx12 build-driver --sdk "$HOME/VulkanSDK/1.4.363.0/macOS"
target/release/nucleon-devtools dx12 build-wine --sdk "$HOME/VulkanSDK/1.4.363.0/macOS"
target/release/nucleon-devtools dx12 probe-driver
target/release/nucleon-devtools dx12 probe-vulkan
target/release/nucleon-devtools dx12 probe-d3d12
target/release/nucleon-devtools dx12 prepare-runtime
```

Use `--sdk` for another local SDK path. The patched native driver retains its
known sparse-depth failures; `probe-driver` reports failures and exits nonzero.
D3D12 probes require a licensed x64 compiler DLL supplied through
`NUCLEON_PROBE_D3DCOMPILER`. `dx12 probe-steam` is an explicit owned Steam-route
probe, requires the local Nucleon bridge and `NUCLEON_STEAM_API`, and uses the
normal Rust prefix/bridge preparation. It does not run a game.

`prepare-runtime` prints an unselected complete generation. Import that path with
`nucleon wine fex install <path> --register` when intentionally selecting it.

## Experimental ARM64EC graphics

```sh
target/release/nucleon-devtools frontend fetch
target/release/nucleon-devtools frontend build --component dxvk --architecture arm64ec
target/release/nucleon-devtools frontend build --component vkd3d --architecture arm64ec
target/release/nucleon-devtools frontend probe --architecture arm64ec --api dx11-vulkan
target/release/nucleon-devtools frontend probe --architecture arm64ec --api dx12
target/release/nucleon-devtools frontend probe-tls
target/release/nucleon-devtools frontend prepare-runtime --base-runtime /path/to/baseline
```

The same probe supports `--architecture x64` controls. ARM64EC DXMT additionally
requires LLVM 15.0.7 and Xcode's Metal compiler; use `frontend doctor`,
`frontend build --component dxmt`, and `frontend probe --api dx11`.
`--native-llvm` overrides the native LLVM prefix. DXMT remains a separate probe;
the paired ARM64EC generation supports only the pinned DXVK/VKD3D x64-guest set.
Assembly reuses Nucleon's production pairing/ABI validator and retains a separate
prefix family. GPU success alone does not certify Steam/gameplay.

## Publisher and diagnostic launches

See [RUNTIME-RELEASES.md](../RUNTIME-RELEASES.md) for the source-inclusive core
publisher. `release build`, `release package` and `release validate` retain the
x64 **and x86** execution gates. The current x86 child-process failure blocks
publication; converting the tooling does not fix it.

```sh
target/release/nucleon-devtools launch dirt --runtime /path/to/complete-runtime --timeout 120
target/release/nucleon-devtools launch dmc5 --runtime /path/to/complete-runtime --api dx12 --timeout 120
```

These diagnostic commands require native Steam already running, the installed
game and a local bridge. Without `--runtime` they use the registered FEX default;
they do not implicitly build or assemble one. `--game-dir`, `--bridge`, `--runner`,
`--work` and `--debug` override diagnostic inputs. They call the normal runner
with private profiles, isolated compatdata and structured receipts. Time limits
start after game spawning. DMC5 settings are locked by their actual canonical
path, backed up durably and restored byte-for-byte after success, failure or
interruption. Steam cloud/user saves remain the real user saves.

Rust build/assembly identities use explicit schema 2 and compact Serde JSON;
Python generation/build hashes are not interchangeable. Existing runtime
schema-1 descriptors and ARM64EC pairing contracts remain compatible.
