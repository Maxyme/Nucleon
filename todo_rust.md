# Runtime tooling split

- [x] Standalone `../wine-fex` Rust builder; no Nucleon source dependency.
- [x] Move Wine/FEX/graphics builds, assembly, recipes, patches and probes.
- [x] Keep Nucleon game diagnostics and Steam-route integration probe here.
- [x] Named/versioned and explicit path/URL `nucleon wine install` interface.
- [x] Builder invokes Nucleon's real installer/inspection CLI for acceptance checks.
- [x] Always run x86 tests; failures temporarily optional and clearly reported.
- [ ] Fix the 32-bit child-process error 998; restore mandatory release policy.
- [ ] Push/configure the separate remote, pin the consumer commit and execute release CI.
- [ ] Requalify ported GPU/Steam/game workflows; CPU validation alone is insufficient.
