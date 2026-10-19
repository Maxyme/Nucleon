# Wine/FEX runtime installation

Runtime builds and publishing now live in the separate [wine-fex repository](../wine-fex/README.md)
(planned GitHub release source: `Maxyme/wine-fex`). Nucleon consumes its artifacts;
it does not compile Wine/FEX, and has no dependency on the builder repository.
The runtime's releases and versions are independent of Nucleon releases.

```sh
nucleon wine install wine-fex --version 0.1.0-rc.1 --register
nucleon wine install ./wine-fex-0.1.0-rc.1-macos-arm64.tar.gz --register
nucleon wine install /path/to/complete-runtime --register
nucleon wine install https://github.com/Maxyme/wine-fex/releases/download/v0.1.0-rc.1/wine-fex-0.1.0-rc.1-macos-arm64.tar.gz --register
```

The named source requires an exact version. It downloads schema-2
`runtime-release.json` from release tag `v<version>`, verifies the metadata version,
component, filename and SHA256, then uses the ordinary managed importer. Explicit
URLs use adjacent release metadata or `--sha256 <digest>`. A local archive's
checksum is computed unless explicitly supplied. `--version` applies only to the
named `wine-fex` source. No version automatically follows latest.

Installation checks the descriptor, ABI, containment and native dependencies,
retains immutable generations and preserves prior selections on import failure.
`--register` selects the experimental FEX Steam tool and leaves the ordinary Intel
Wine default alone. Without it the runtime is only installed.

```sh
nucleon wine fex verify
nucleon wine fex status
nucleon wine inspect /path/to/runtime --json
nucleon launch 690790 --emulator fex
nucleon launch 690790 --wine fex-preview
```

The existing `wine fex install` command remains a compatibility interface for
explicit paths/URLs. Steam bridge/helper components remain separately installed.
The core package includes native Tolkara Wine, both FEX emulators and companions,
WineD3D, provenance and notices. It excludes developer graphics add-ons, Vulkan,
Apple GPTK, Microsoft game DLLs and Valve/Steam binaries. Core installation does
not certify DX12/DXVK or gameplay.

## Builder validation policy

The builder always runs x64 and x86 workloads. Temporarily, the existing x86
`CreateProcessW` error 998 is recorded as an optional failure and does not prevent
preview publishing. x64 execution, import/integrity/native closure and cleanup
remain mandatory. `--require-x86` in the builder restores the full gate. Release
notes and metadata disclose the result; optional failure is not reported as pass.
The earlier blocked results in `tools/arm64-fex-release-validation.json` and
`tools/rust-tooling-validation.json` are historical evidence from before this policy.

The builder repository is local until separately pushed and configured. The named
version command cannot download a release that has not been published. Local
paths already exercise the same installation/registration flow. Publisher CI
pins a Nucleon revision and invokes this CLI for validation, avoiding duplicate
installer/ABI rules in the builder. The inspection JSON envelope is schema 1.
