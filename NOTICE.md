# Third-party notices

Copyright (c) 2026 Nucleon Contributors. Nucleon is distributed under GNU GPL
version 3 only (`GPL-3.0-only`); see [LICENSE](LICENSE).

## Source and dependencies

- Rust dependencies retain their respective licenses and copyright notices.
  Versions are recorded in [Cargo.lock](Cargo.lock).
- Wine patches in [assets/patches](assets/patches/) retain Wine's LGPL terms.
  The Rosetta WoW64 workaround derives from dttdrv/wiage's Wine/CodeWeavers
  implementation (CW HACK 20760), under LGPL-2.1-or-later. The Steam redirect
  modifies Wine's LGPL-2.1-or-later loader. Patched runtime copies retain Wine's
  license, authors, applied patches and build receipts.

## Runtime and upscaler components

- Imported Wine/FEX runtimes, Mesa, DXVK, DXMT, VKD3D-Proton and D7VK retain the
  licenses and notices supplied with their source and runtime packages.
- `lsteamclient` binaries require their exact source revision and applicable
  Steamworks SDK notices. Proton-derived `steam.exe` helpers retain BSD-3-Clause
  terms and the notices for any Steamworks, OpenVR and Wine headers used.
- Optional upscaler providers use OptiScaler, its MetalFX fork, OptiPatcher and
  native FSR4 sources. Pinned releases and revisions are recorded in
  [dependencies.json](tools/upscalers/dependencies.json); provider installation
  retains upstream license and notice files.
- Valve client libraries and Apple GPTK/D3DMetal retain their own terms.
  Nucleon's license does not grant redistribution rights to third-party
  components.
