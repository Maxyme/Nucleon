# Prebuilt ARM64 Wine/FEX runtime releases

The publisher compiles the patched Wine/FEX runtime once. Users install the
prebuilt archive with Nucleon; no Python, compiler, SDK, source checkout or
Rosetta installation is needed for this runtime's installation and CPU execution.
Steam integration still requires the separately installed Nucleon Steam bridge
and helper. This does not certify that a game works.

## Package scope

The initial release is the **experimental Wine/FEX core**: native ARM64 Tolkara
Wine, the x64 ARM64EC FEX emulator, the x86 WoW64 FEX emulator, both matching
native companions, Wine's built-in WineD3D, notices and build provenance.
Windows PE images have debug sections stripped; native Mach-O layout is retained
and those binaries are ad-hoc signed. Exported entry points and Windows unwind
metadata are retained.
It uses the source pins and downstream patches in
`assets/wine/recipes/arm64-fex.toml`, including the cursor-registry correction.
The default graphics frontend remains x64; this is not the separate experimental
ARM64EC DXVK/VKD3D graphics generation.

Graphics add-ons, Vulkan loaders/drivers, reference-app graphics binaries, Apple
GPTK/D3DMetal, Microsoft game DLLs and Valve/Steam libraries are not packaged.
This core package does not provide the full graphics stack used by the local
DiRT/DMC5 experiments. In particular, the current core build disables Vulkan;
its release must not be advertised as a ready-to-run DX12/DXVK runtime.

## Current validation (2026-10-16)

Publisher regression tests pass, and the real archive passed managed import,
full integrity/native-dependency verification and the owned x64 workload. The
x86 workload passed memory, threading and exception checks, but
`CreateProcessW` failed with error 998. The same failure reproduced in an
unstripped runtime and in a control with exception handling removed. This is a
current runtime blocker, not a packaging-only failure. The workflow deliberately
fails its publication gate until the x86 workload also passes; this change has
not published a runtime release. See `tools/arm64-fex-release-validation.json`.

## Users

Download the runtime archive and `SHA256SUMS.txt` from its GitHub prerelease.
Use the exact checksum listed for the runtime archive, for example:

```sh
nucleon wine fex install ./nucleon-arm64-fex-core-0.1.0-rc.1-macos-arm64.tar.gz \
  --sha256 <published-64-character-SHA256> --register
nucleon wine fex verify
nucleon wine fex status
```

The same command accepts an HTTPS release-asset URL instead of the local file.
Installation verifies the checksum, runtime ABI and native dependencies, then
retains an immutable generation. `--register` selects it only for the dedicated
experimental FEX Steam tool, without changing ordinary Intel/Rosetta Wine.

## Publisher

On native Apple Silicon macOS, install Xcode Command Line Tools, Rust, Python
3.12+, current Bison/Flex, CMake and Ninja. Build the CLI and run:

```sh
cargo build --release --locked -p nucleon-cli
python3 tools/arm64_fex_release.py build --version 0.1.0-rc.1 \
  --work target/runtime-release --output target/runtime-release-assets \
  --nucleon target/release/nucleon --jobs 2
```

Use a new output directory for each attempt. `package` can replace `build` to
repackage an unchanged publisher build; it rejects source/product changes since
that build. It cannot package a developer reference-app generation as a release.
The publisher's compiler archive is checksum-pinned; it is not shipped in the
runtime. The workflow uses a native ARM64 `macos-15` runner and verifies the host
architecture before building.

The output contains:

- An installable runtime `.tar.gz` with a root `nucleon-runtime.toml`.
- A corresponding-sources `.tar.gz`: patched Wine, FEX and initialized FEX
  submodules, the original shared-data helper source tree, and Nucleon's sources,
  patches, build scripts and notices. Each exported source file is hashed in
  `SOURCE-MANIFEST.json`. Source archives contain working-tree bytes, not just an
  unpatched upstream Git revision. Ignored build products and arbitrary untracked
  publisher files are excluded.
- `runtime-release.json`, `RELEASE-NOTES.md`, `SHA256SUMS.txt` and validation logs.

The source bundle includes the complete patched compilation inputs and the
scripts controlling compilation and installation. Its `sources/nucleon` tree
contains the build recipes and publisher command above; that command fetches
exact upstream revisions and the pinned compiler. The bundled patched sources
remain available independently of those upstream servers for manual rebuilding
with Wine's configure/make and FEX's CMake recipes in `tools/arm64_fex.py`.

Before publishing, the packager imports the archive with the normal CLI into a
private temporary support directory, runs full installed integrity/dependency
verification, executes the owned x64 and x86 memory/thread/exception/child-process
workloads from that relocated installation, and waits for its Wine server to
exit. These checks never use Steam or installed games. Failure publishes no
release asset directory; publisher logs remain under the work directory.

## Versioning and automation

Commit the recipe, patches, publisher code and notices together. Create and push
an explicit tag such as `runtime-arm64-fex-v0.1.0-rc.1` on that commit to trigger
`.github/workflows/arm64-fex-runtime.yml`. The workflow publishes a GitHub
**prerelease**, never marks it latest, and includes binary/source checksums. It
uploads all assets to a draft before making the release public, so an upload
failure cannot expose a binary-only release.
The runtime tag is independent of application `v*` tags and its package version
is independent of the content-based managed-install generation.

Manual workflow dispatch builds/tests downloadable CI artifacts without creating
a tag or publishing a release. No release has been published merely by adding
this workflow. Failed releases are not automatically promoted or overwritten.

## Distribution notices

Wine and Wine-derived patches retain LGPL-2.1-or-later terms. Releases include
corresponding patched source and build scripts, upstream copyright/license
notices, and the recorded downstream patch set. Modified Wine/FEX compilation files receive
prominent Nucleon modification notices and dates before compilation. The borrowed
shared-data header is copied unchanged with its original notices. FEX retains MIT terms, with its
bundled dependencies' own notices and the LLVM/MinGW runtime notices; Nucleon's scripts retain GPL-3.0-only terms.
A source snapshot is not permission to redistribute unrelated proprietary
binaries. Extending the package with graphics or SDK components requires adding
their corresponding source and/or notices as applicable to those components.
