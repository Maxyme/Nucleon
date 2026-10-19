> 2026-10-19 split: build and publisher implementations now live in `../wine-fex`; Nucleon retains installation/ABI validation and integration diagnostics. The builder calls a Nucleon executable for acceptance checks and has no Cargo dependency on it. Its 32-bit test now runs as an optional preview gate. The review below records the preceding migration.

# Code simplification and library review

Implemented on the ARM64/FEX branch alongside per-launch runtime selection and
the Rust developer-tool migration. The goal is fewer implementations of the same
policy, while preserving the ABI, source-export, process-ownership and publication
checks already established by the branch.

## Changes made

| Area | Reuse or library | Result |
| --- | --- | --- |
| Wine/emulator selection | Shared `runtime_selection` API | CLI preflight and runner use the same exact-ID/path resolver and mode constraints. Legacy fuzzy Wine-management queries remain a separate compatibility interface. |
| Runtime import validation | Existing `managed_runtime` importer with an explicit destination | Publisher relocation uses the production installer without spawning another CLI or modifying global environment. |
| ARM64EC assembly | Existing production runtime/pairing validator | The assembler no longer maintains a second provenance/ABI acceptance implementation. |
| Downloads and archive extraction | Existing downloader plus `tar`, `zip`, `flate2`, `zstd`, `lzma-rs` | Developer tools and Wine installation share bounded downloads, checksum handling and extraction; Wine no longer shells out to tar for XZ packages. |
| Copying and hashing | Shared `artifacts` primitives and existing SHA256 helper | Removed the Wine builder's separate hash loop, downloader and recursive copier. Developer scripts no longer each own atomic-copy/JSON helpers. |
| PE image identities | Existing `object` dependency | Timestamp/image-size/checksum come from validated PE headers rather than another byte-offset parser. |
| App bundle versions | Existing `plist` dependency | Replaced XML substring scanning; binary and XML plists use the same parser. |
| Filesystem traversal | Existing `walkdir` dependency | Stable walks and explicit symlink policy replace recursive walkers. |
| Runtime release versions | `semver` | Replaces the release-name regular expression with explicit version parsing and a no-build-metadata policy. |
| Meson argument quoting | `shlex` | No custom shell quoting; subprocess arguments themselves are passed directly as data. |
| Developer commands | Existing `clap`/`serde`/`toml` | One typed command tree and typed recipes replace repeated argparse/TOML boilerplate. Descriptors are serialized with TOML rather than assembled line by line. |
| Process lifetime and locks | Existing `OwnedChild` plus `rustix` | One implementation of group ownership, deadlines, interruption, cleanup and configuration locking. Cleanup can run after cancellation. |
| Diagnostic launchers | Existing normal runner | Private profiles, receipts, prefix preparation and cleanup remain shared. Only explicit game-specific configuration handling stays in the wrapper. |

The new dependencies are `lzma-rs` in core and `semver`/`shlex` in developer
tooling; their small transitive dependencies are locked in Cargo.lock. The other
libraries were already dependencies of the workspace. References:
[tar](https://docs.rs/tar/0.4.46/tar/struct.Builder.html),
[plist](https://docs.rs/plist/latest/plist/enum.Value.html),
[walkdir](https://docs.rs/walkdir/2.5.0/walkdir/),
[lzma-rs](https://docs.rs/lzma-rs/0.3.0/lzma_rs/fn.xz_decompress.html),
[semver](https://docs.rs/semver/1.0.28/semver/struct.Version.html).

## What was deliberately retained

Archive libraries provide encoding/decoding, not Nucleon's complete policy.
Keep the small checks for bounded extraction, contained symlinks, privileged
entries, pinned submodules, patched working-tree source snapshots, stable
metadata, native dependency closure, and atomic publication. Deleting those
checks in favor of a generic `unpack`/`copy_dir` call would change behavior.

`git2` would add a native dependency while requiring new implementations of
Git's intent-to-add, submodule and working-tree export semantics. The current
Git CLI is the source of truth and runs under the shared bounded process helper.

`wait-timeout`, `duct`, or an async runtime would not replace the requirement to
own and clean up descendant process groups, preserve the leader's identity until
cleanup, and respond to interruption. Reuse the existing tested process owner;
do not introduce another supervisor solely to shorten call sites.

The generic binary-image reader supports swapped/fat64 Mach-O inputs and strict
truncation checks. A proposed library replacement must pass that existing test
matrix; the current object-library use does not justify relaxing those cases.

DMC5's temporary configuration edit changes three exact keys and preserves
unrelated bytes/line endings. A general INI serializer would add normalization
and round-trip behavior to review. The original bytes remain the authoritative
backup and are restored even on failure/interruption.

## Code size and next steps

Rust is more verbose than Python: this migration is **not a net reduction in
physical source lines**. The retired Python implementation/test files contained
about 3,136 lines; the Rust developer crate also includes a unified CLI, explicit
error handling, shared execution and regression tests. Its benefits are shared
policy, typed recipes, direct use of the installer/validator, and one maintained
implementation. The historical MacRunner loader build route was retired rather
than ported because it never passed Windows execution. Shared-data source and
reference graphics pins needed by the supported Tolkara route remain.

Further useful reduction should consolidate repeated command/receipt setup when
new workflows demonstrate the same needs, or retire experiments no longer used.
A general workflow DSL or additional broad utility framework would currently add
another abstraction to maintain. Avoid expanding the end-user CLI with build,
publisher or probe internals; those stay in the developer crate.

## Validation limits

Portable regression coverage includes source export with patched/intent-added
files and pinned submodules, immutable snapshots, deterministic archive metadata,
symlink rejection, PE-only stripping, all-guest build identity coverage, failed
publication, cleanup error precedence, argument quoting, environment filtering,
configuration locking/restoration and private diagnostic wiring.

Real local checks exercised Rust core packaging, relocated managed import and
verification, x64/x86 CPU execution, native companion ABI behavior, and assembly
of a paired ARM64EC generation accepted by the production validator. The x64 CPU
checks passed; x86 reproduced child-process error 998 and still blocks release.
The full updated from-source publisher workflow and the newly ported GPU/Steam
probe commands have not received a new execution qualification. No Steam/game
launch was performed while implementing the migration.

The exact checks and limits are recorded in
[tools/rust-tooling-validation.json](tools/rust-tooling-validation.json).
