# Third-party provenance

Nucleon is distributed under GNU GPL version 3 only (`GPL-3.0-only`), effective
2026-10-13. Copyright (c) 2026 Nucleon Contributors. See `LICENSE` for the full
terms, including the warranty disclaimer and source distribution requirements.


## Runtime components

- Wine and Wine-source patches retain Wine's LGPL terms and source notices.
  The Rosetta WoW64 workaround is derived from dttdrv/wiage's Wine/CodeWeavers
  implementation (CW HACK 20760), under LGPL-2.1-or-later. The Steam redirect
  modifies Wine's LGPL-2.1-or-later loader. Native Rust build orchestration is
  covered by Nucleon's GPL-3.0-only license. Built runtime copies retain the
  applied patches, Wine license, authors, source references, and build receipts.
- The experimental ARM64 runtime uses the pinned MacRunner/HyperBridge and
  Tolkara forks listed in `assets/wine/recipes/arm64-fex.toml`. FEX and
  HyperBridge retain their MIT notices. The shared-data read decoder is copied
  unchanged at build time from MacRunner's Wine fork, under Wine's LGPL terms;
  Nucleon's downstream integration patch records that source and its checksum.
  The optional DXMT test uses the reference archive's graphics components and
  records their hashes; it does not build those components from source.
- `lsteamclient` binaries require their exact source revision and applicable
  Steamworks SDK notices.
- The `steam.exe` helper derived from Proton's steam-helper uses BSD-3-Clause,
  with separately licensed Steamworks, OpenVR, and Wine headers as applicable.
- Valve client libraries, Apple GPTK/D3DMetal, and graphics translation runtimes
  retain their own terms. Nucleon's license does not grant redistribution rights
  to those components.
- The DX12 prototype builds Mesa KosmicKrisp with a pinned Steamac patch series.
  Mesa retains its file-specific licenses and notices; Steamac is Apache-2.0,
  and imported upstream patches retain their own notices. Source pins, patch
  hashes and native build receipts are kept with the private driver. Official
  VKD3D-Proton releases retain LGPL-2.1-or-later terms; DXVK retains its
  zlib/libpng license. The shader probe can use a separately licensed Microsoft
  compiler from a local game;
  that proprietary DLL is not redistributed by this repository.
