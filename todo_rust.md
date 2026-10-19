# Python-to-Rust tooling migration

The maintained Tolkara build, packaging, assembly, probe and diagnostic workflows
are implemented in `crates/nucleon-devtools`. The eight Python implementations
and five test files were retired after the portable tests, a final Python
reference-suite pass, and the native/static checks below. Their original source
remains in Git. The obsolete MacRunner loader build route was retired; the
supported runtime remains the pinned Tolkara Wine/FEX pair.

- [x] Separate developer CLI; keep installation/ordinary launches in `nucleon`.
- [x] Shared process ownership, timeouts, interruption and cleanup.
- [x] Typed recipes, pinned source/toolchain checks and ordered patch overlays.
- [x] Shared bounded downloads/extraction, hashing, copying and atomic JSON.
- [x] Wine/FEX build orchestration and CPU/shared-data/window probes.
- [x] Native companion ABI probe, including file mapping and FD ownership.
- [x] Patched tracked source snapshots and initialized pinned submodule exports.
- [x] Deterministic archive metadata, symlink/privilege checks, PE-only stripping,
  native signing, source notices and compiler/runtime notices.
- [x] Publisher relocation through the normal managed importer and full verify.
- [x] Mandatory x64 and x86 execution gates; no partial asset publication.
- [x] DX12 driver/runtime assembly and graphics/Steam probe orchestration.
- [x] Experimental frontend builds, CRT/TLS checks and paired assembly using the
  production validator; retain separate prefix families and immutable generations.
- [x] Diagnostic wrappers around the ordinary runner, private profiles/receipts,
  per-game runtime bounds, actual-configuration locks, durable settings backups
  and exact restoration after errors/interruption.
- [x] Rust portable regression tests, active command documentation and CI entry
  points; remove Python implementation/test callers from active workflows.

Rust developer build/generation identities use schema 2 and canonical compact
Serde JSON rather than reproducing Python's JSON whitespace. Existing runtime
descriptors and graphics pairing contracts retain schema 1. Python publisher
receipts cannot stand in for a Rust publisher build identity.

Validation completed locally:

- Final Python reference suite: 27 passed, one optional native test skipped.
- Rust regression tests, formatting and strict lint checks.
- Real core archive creation/import, integrity/native dependency checks,
  x64 CPU execution and prefix cleanup passed.
- x86 execution reproduced `CreateProcessW` error 998. Publication remains blocked.
- The Rust native companion ABI probe passed.
- Rust assembly produced a generation accepted by the production ARM64EC pairing
  validator; managed-import/staging qualification is recorded with the task results.
- CLI preflight checks exercised explicit runtime selection, emulator conflicts,
  native profile routing, stale unused backend paths and stale Steam-tool pins
  without creating a prefix or launching Wine.

Remaining runtime/release qualification (not claims made by this migration):

- [ ] Execute the updated from-source publisher workflow on its native CI host.
- [ ] Fix the x86 runtime defect; do not relax its release gate.
- [ ] Requalify the ported GPU/Steam probe commands before relying on new results
  or promoting experimental graphics. Historical probe/game receipts remain dated
  evidence from their original implementations.
- [ ] Define and validate a redistributable game-usable graphics package; the
  current core package still omits the fuller local developer graphics stack.

Upstream FEX CMake still requires Python 3.9+, and Meson graphics builds retain
Python dependencies. Replacing Nucleon's orchestration does not remove those
third-party build requirements. See [tools/README.md](tools/README.md) and
[CODE-SIMPLIFICATION.md](CODE-SIMPLIFICATION.md).
