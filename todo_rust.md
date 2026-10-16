# Python-to-Rust tooling migration

## Findings

Converting Nucleon's Python tooling to Rust is practical. The current inventory
contains eight implementation scripts (about 2,474 lines) and five test files
(about 662 lines). Most of this code coordinates external builds, Git, packaging,
and probes.

The main benefits are typed recipes and receipts, shared runtime validation, and
less duplicated utility code. Compilation dominates build time, so changing the
orchestration language is unlikely to substantially speed up builds.

| Scripts | Purpose | Complexity |
| --- | --- | --- |
| `tools/arm64_fex.py` | Source/toolchain fetching, Wine/FEX builds, CPU probes | Medium–high |
| `tools/arm64_fex_release.py` | Source snapshots, packaging, release validation | High; provenance and publication guarantees |
| `tools/arm64_fex_dx12.py` | Vulkan builds, runtime assembly, Steam/DX12 probes | High |
| `tools/arm64ec_graphics.py`, `tools/arm64ec_runtime.py` | Frontend builds, GPU/TLS probes, paired runtime assembly | Medium–high |
| `tools/nucleon_fex.py`, `tools/launch_dirt_fex.py`, `tools/launch_dmc5_fex.py` | Diagnostic wrappers around the Rust runner | Low–medium |

## Architecture

- [ ] Add a separate developer-tool crate, such as `nucleon-devtools`, invoked
  through `cargo run`.
- [ ] Keep publisher and experimental build/probe commands in that crate; retain
  installation and normal game launching in `nucleon`.
- [ ] Reuse existing Rust process-group cleanup, atomic writes, runtime inspection,
  managed installation/verification, archive handling, and Wine build utilities.
  Refactor private helpers selectively where sharing is useful.
- [ ] Use typed recipe and receipt models with explicit schema versions.
- [ ] Preserve the separation between source fetching, builds, probes, runtime
  selection, and game launches. Builds must not implicitly register or select a
  runtime or launch Steam/games.

## Migration order

### 1. Shared foundations

- [ ] Port typed recipes and receipts.
- [ ] Add logged external-command execution with timeouts, interruption handling,
  and cleanup of owned process groups.
- [ ] Port Git revision/submodule checks and ordered patch-overlay verification.
- [ ] Reuse hashing and atomic file replacement; port workspace/configuration locks.
- [ ] Preserve environment filtering and avoid changing global process environment.
- [ ] Establish compatibility fixtures for existing receipts and identity hashes.

### 2. Core Wine/FEX builds and CPU probes

- [ ] Port source/toolchain fetching and checksum verification from `arm64_fex.py`.
- [ ] Port the supported Wine/FEX build variants and matching native companions.
- [ ] Preserve source modification notices and build provenance.
- [ ] Port CPU probes, result reporting, negative controls, and prefix-scoped cleanup.
- [ ] Keep builds isolated from installed runtimes and existing game prefixes.

### 3. Release packaging

- [ ] Port `arm64_fex_release.py` and its regression tests.
- [ ] Export patched tracked working-tree bytes, including intent-added helpers and
  initialized submodules at their pinned revisions.
- [ ] Exclude arbitrary untracked files and build products from source exports.
- [ ] Take immutable source snapshots before hashing and archiving.
- [ ] Preserve source manifests, license notices, and publisher build identities.
- [ ] Preserve deterministic archive metadata and rejection of escaping/dangling
  symlinks, privileged entries, and unsupported entry types.
- [ ] Strip PE debug information only; preserve native Mach-O layout and re-sign
  native images.
- [ ] Validate through the normal managed installer in an isolated support directory,
  then perform full verification and x64/x86 execution probes.
- [ ] Publish the output asset directory only after all required validation passes.
- [ ] Retain diagnostic logs on failure and preserve the primary error if cleanup
  also fails.

### 4. Experimental graphics tooling

- [ ] Port DX12/Vulkan build orchestration and runtime assembly.
- [ ] Port ARM64EC frontend builds and paired-component identity checks.
- [ ] Port TLS and GPU probes, loaded-image checks, and negative controls.
- [ ] Preserve separate experimental prefix families and immutable generations.

### 5. Diagnostic launch wrappers

- [ ] Replace the shared Python launch wrapper using the existing Rust runner.
- [ ] Preserve diagnostic profiles, structured receipts, logs, and timeout semantics.
- [ ] Port DiRT and DMC5 diagnostic entry points where they add capabilities beyond
  the normal CLI.
- [ ] Preserve DMC5 configuration locking, exact-byte backup/restoration, durable
  recovery files, and restoration on errors or interruption.

## Validation and CI transition

- [ ] Translate the five Python test files into Rust unit/integration tests,
  preserving meaningful behavior checks.
- [ ] Compare Python and Rust source manifests, descriptor contents, archive
  entries/modes/links, and failure behavior using fixture repositories.
- [ ] Decide whether to reproduce existing identity serialization exactly or
  introduce a versioned identity format. Python's JSON encoding contributes to
  generation/build hashes; equivalent Rust data can otherwise hash differently.
- [ ] Compare extracted archive contents and metadata; require byte-identical
  archives only if that is an explicit compatibility requirement.
- [ ] Run relocated-install verification and both guest-ABI execution gates before
  replacing the publisher entry point in CI.
- [ ] Run portable tooling tests on Linux; keep macOS-native build/sign/execute
  validation on an Apple Silicon Mac.
- [ ] Update workflow commands, source-export lists, build-identity inputs,
  documentation, and developer instructions when switching implementations.
- [ ] Remove replaced Python scripts only after their Rust replacements pass the
  required checks and all callers have migrated.

## Constraints and existing blocker

- Rewriting orchestration in Rust does not remove the macOS requirement for
  native runtime compilation, signing, and execution validation.
- Graphics builds use Meson, so replacing Nucleon's Python scripts does not remove
  every Python dependency from the external build toolchain.
- The currently documented x86 `CreateProcessW` failure (Win32 error 998) remains
  a release blocker. Preserve the failing validation gate throughout migration;
  the language rewrite is not a fix for that runtime defect.
- Migration work must not publish a release or launch Steam/games implicitly.
