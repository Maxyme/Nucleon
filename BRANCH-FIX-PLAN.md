# Branch recommendation review and resolution plan

Reviewed 2026-10-16 against `codex/arm64-fex-dx12`, commit `8943f2e`.
This document plans implementation; it does not change runtime behavior.

## Assessment

| Recommendation | Verdict | Required adjustment |
| --- | --- | --- |
| 1. Shared typed Vulkan ICD resolution | Accept, medium priority | Five sites exist, including `runtime.rs`. `configuration.rs` already uses typed deserialization. Preserve API-version checks and bundled-runtime containment restrictions. |
| 2. ABI marker comparison | Accept, medium priority | Deserialize `ExecutionMode` instead of comparing JSON text. Keep malformed, unreadable, and incompatible markers fatal. |
| 3. Temporary registry cleanup | Accept, medium priority | Apply to both registry import paths. Cleanup may be best effort only after successful import; readiness-marker writes must still succeed. |
| 4. Executable canonicalization context | Accept, low priority | Improve diagnostics while preserving early failure and current argument handling. |
| 5. Tests in restricted environments | Accept the isolation goal, revise approach | Use deterministic fixtures. Keep any real-Steam smoke test explicit and optional; do not silently swallow every read error. |
| 6. ARM64EC production frontends | Accept as a separate experimental rollout | A descriptor flag and broader PE check are insufficient. Require a coherent component contract, artifact provenance, correctness validation, and then controlled benchmarks. |

## 1. Consolidate Vulkan parsing and resolution

Status: implemented on 2026-10-16. All five callers below use the shared typed
resolver. Regression coverage includes bare/absolute/relative paths, malformed
documents and versions, missing/nonfile libraries, native architecture and
package containment. Verification: 190 core tests passed (2 ignored) with an
isolated support directory and serial execution; all 6 runner configuration
tests, core/runner Clippy checks and formatting checks passed. An initial core
run using the host support directory had two support-directory-dependent failures;
both passed in the isolated run. GPU/game validation was not run for this change.

Affected code:

- `crates/nucleon-core/src/configuration.rs::vulkan_manifest`
- `crates/nucleon-core/src/launch.rs::GraphicsComponents::discover`
- `crates/nucleon-core/src/managed_runtime.rs::check_runtime`
- `crates/nucleon-core/src/runtime_checks.rs::vulkan_arch`
- `crates/nucleon-core/src/runtime.rs::ResolvedRuntime::from_descriptor`

The duplication is confirmed, but the proposed panic mechanism is overstated:
launch discovery canonicalizes the manifest before calling `parent().unwrap()`,
and descriptor inspection resolves bundled manifests to canonical paths before
the other unchecked parent accesses. A bare relative filename normally has an
empty parent path in Rust. Defensive handling still belongs in the shared helper.

Implementation:

1. Extract crate-private typed manifest structs and one reader/resolver in
   `configuration.rs`, using the existing `json()` diagnostics. Return both the
   declared library path and canonical resolved path, plus typed API-version
   metadata needed by compatibility checks. Allow vendor extension fields.
2. Reject an empty `library_path`; resolve relative paths beside the manifest,
   including bare relative manifest filenames. Canonicalize absolute and relative
   library paths consistently, with manifest/library context on failures.
3. Preserve the distinction between configured-file validation and compatibility:
   callers requiring Vulkan 1.3 still reject missing/insufficient API versions;
   ordinary manifest validation need not introduce that requirement. Avoid
   permissive version parsing that drops invalid components.
4. Replace all five parsers. Preserve source labels such as `VK_DRIVER_FILES`,
   unreadable-library diagnostics, and boolean failure behavior in discovery
   compatibility checks.
5. In descriptor inspection, continue rejecting absolute declared library paths
   and canonical paths outside the runtime root, including symlink escapes.
   A generic resolver alone does not enforce this package policy.

Acceptance: shared tests cover absolute/relative libraries, bare relative
manifest names without changing process-wide current directory, empty/missing/
wrong-type fields, invalid/trailing JSON, extension fields, missing libraries,
API-version boundaries and malformed versions. Existing native-architecture and
package-containment tests must continue to pass, with regressions added where
coverage is missing. All five callers use the common parser.

## 2. Compare ABI ownership semantically

Status: implemented on 2026-10-16. Existing markers deserialize as `ExecutionMode`
and are left untouched when compatible, including whitespace and JSON escapes.
Missing eligible markers are written atomically after one read; malformed,
incompatible and unreadable markers fail with their path in the error. Regression
tests cover those cases and the existing-prefix adoption restrictions. Verification:
193 core tests passed (2 ignored) with an isolated support directory and serial
execution; core/runner Clippy, formatting and diff whitespace checks passed.

In `prefix.rs::ensure_prefix_for_plan`, parse existing marker contents as
`ExecutionMode` and compare to `runtime.mode`. JSON deserialization accepts
surrounding whitespace while rejecting malformed or unknown values. Include the
marker path in parse/read errors and retain the incompatible-prefix diagnostic.

Avoid the second raw string comparison/read that currently decides whether to
rewrite the marker. Record whether the marker was absent; publish it atomically
when needed. Retain the rule preventing adoption of an unidentified existing
native prefix, and retain `prefix_session.rs` family/ABI ownership checks.

Acceptance: exact JSON, trailing newline and surrounding whitespace are accepted
for the matching mode; a different mode, invalid JSON, unknown enum and unreadable
marker fail without rewriting ownership. Missing-marker behavior is unchanged.

## 3. Decouple successful registry import from temporary-file cleanup

Status: implemented on 2026-10-16. Both import paths share completion handling
that ignores an absent temporary file, logs other cleanup failures and requires
atomic readiness-marker publication to succeed. Regression checks cover successful
cleanup, missing files, deterministic cleanup failure, marker-write failure and
failed import without readiness. Verification: 195 core tests passed (2 ignored)
with an isolated support directory and serial execution; core/runner Clippy,
formatting and diff whitespace checks passed.

Update both `ensure_prefix_for_plan` and `configure_prefix_registry` in `prefix.rs`.
After successful `regedit`, attempt to remove `nucleon_tweaks.reg`: ignore absence
and log other removal failures without failing preparation. Then publish the
configured marker atomically. Keep registry execution and marker-write errors
fatal. Preserve staging receipt publication rules and retry behavior.

Acceptance: failed import never publishes readiness; successful import with a
missing or undeletable temporary file can publish readiness; failed marker writes
remain errors. Use focused error injection or a small cleanup seam rather than
unreliable antivirus simulation or permission assumptions.

## 4. Improve executable-resolution errors

Add `with_context` around `canonicalize()` in `LaunchPlan::wine_args`, including
the original executable argument. Continue rejecting unresolved absolute paths
before invoking the Steam helper. The proposed change improves the error; it
does not delegate resolution to Wine. Preserve Windows-path arguments, relative
argument behavior, argument boundaries and direct-launch behavior.

Acceptance: extend the existing helper argument test with missing absolute paths
and broken symlinks, asserting actionable path context. Existing space-containing
paths and arguments continue to work.

## 5. Isolate tests from installed Steam and bridge files

The two reported sandbox failures did not reproduce in this review: both targeted
tests passed. The bridge test took 23.80 seconds and depends on the installed
bridge directory; the VDF test depends on the user's Steam configuration. These
remain unsuitable dependencies for deterministic unit tests.

1. Make VDF parsing coverage fixture-based. Retain a real-config smoke test only
   as an explicitly ignored/opt-in integration check, with clear unavailable-file
   diagnostics. If it skips inaccessible files, limit skips to known absence or
   access-denied cases and report them; still fail on parse errors after reading.
   The suggested `if let Ok(...)` would hide unrelated I/O failures.
2. Change `test_stage_bridge_libraries_structure` to call the existing
   `stage_bridge_libraries_from` seam with a temporary bridge directory and prefix.
   Test directory creation without installed assets; existing fixture tests cover
   representative copies and architecture-specific layout.
3. Do not mutate `HOME` or `NUCLEON_SUPPORT_DIR` inside parallel tests. Use explicit
   temporary paths; use a subprocess-scoped support directory only for tests that
   must exercise global-path wrappers.

The claim that this structure test directly requires a writable support directory
is not established by the inspected path: it reads the bridge from that directory
and writes its prefix/receipt under the temporary prefix. Investigate a concrete
failure trace before changing production support-directory behavior.

Acceptance: unit coverage passes without Steam or an installed Nucleon bridge,
without accessing user-library fixtures, and without requiring writable user
support paths. Optional host integration coverage is clearly distinguished from
unit-test results.

## 6. Introduce ARM64EC graphics behind an explicit experimental contract

`tools/arm64ec-graphics-validation.md` records GPU and TLS probe success, including
negative controls and a paired DXMT/native Metal experiment. It explicitly does
not establish Steam compatibility, sustained gameplay, or performance. Retain
the normal rejection until an opt-in path implements the contract below.

1. Define a typed graphics frontend ABI (`x64` / `arm64ec`) with backward-compatible
   default `x64`. Prefer placing it under graphics configuration so it cannot be
   confused with guest executable, Steam helper, or core Wine ABI. Specify whether
   it applies to the entire declared graphics set; reject mixed sets initially.
   Restrict ARM64EC to the validated ARM64/FEX x64-guest configuration and explicitly
   decide how unsupported 32-bit selection fails. Update descriptor documentation,
   fixtures, packaging and managed-import validation together.
2. Add a graphics-specific ABI matcher; keep generic `pe()` strict for ordinary
   x86/x64 guest modules, Steam helpers and bridges. Recognize ARM64EC machine
   headers and AMD64 headers carrying CHPE metadata. Continue rejecting plain
   ARM64, ARM64X and malformed images unless separately validated.
3. Apply the contract to every selected graphics DLL: DXVK/DXGI, VKD3D/core and
   DXMT/winemetal as applicable. Preserve paired native Unix companions and reject
   environment overrides that substitute an incompatible component. Require the
   tested Darwin CRT/TLS builds and provenance; a flag is not evidence of pairing.
4. Validate packaging, backend availability, explicit selection, staging,
   DLL override rules, rollback and cache/receipt invalidation with the new
   component set. Keep ABI choices out of unrelated prefix ownership unless a
   concrete prefix compatibility change requires it.
5. Repeat GPU/TLS positive and negative controls through the normal launch path,
   verifying actual loaded component identities. Repeat Steam positive/negative
   controls, saves, video, controllers, repeated launches and sustained DMC5/DiRT
   gameplay. Track the earlier DMC5 unexpected exit and sparse-depth failures;
   classify them with evidence rather than declaring them resolved by probes.
6. Only after correctness passes, benchmark a fixed DMC5 gameplay scene against
   AMD64 builds of the same releases. Hold Wine/FEX, driver, settings, resolution,
   synchronization and cache policy constant; separate warmup/cold-cache results,
   alternate run order and repeat runs. Report frame-time distributions, p95/p99,
   FPS and variability. Claim latency improvements only if measured directly.
   Keep software memory ordering and shared-data emulation enabled.
7. Publish validation receipts and measured results before deciding on production
   promotion. Retain the x64 default and a tested rollback path until that decision.

Acceptance: old descriptors behave identically; opt-in matching sets pass while
mixed/wrong ABI sets fail before prefix mutation; normal-launch identity and
correctness controls pass. Performance benefits remain unproven until measured.

## Delivery order and verification

1. First change: deterministic test fixtures (item 5), establishing a reliable
   baseline without local installations.
2. Second change: typed ICD resolution (item 1), including descriptor containment
   and Vulkan-version regression checks.
3. Third change: prefix ownership/cleanup fixes and executable diagnostics
   (items 2–4), with focused failure-path coverage.
4. Separate experimental changes: descriptor contract and guarded graphics ABI
   selection, then packaged integration validation, then gameplay and benchmarks
   (item 6). Production promotion is a later evidence-based decision.

For Rust changes, run focused tests first, then the core suite and runner
configuration-failure integration tests, formatting and appropriate lint checks.
Keep real-GPU/game validation separate from unit-test success. This review ran
only the two targeted existing tests mentioned above; it did not rerun the GPU
experiments, launch games, or establish a full-suite baseline.
