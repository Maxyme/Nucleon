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
- `lsteamclient` binaries require their exact source revision and applicable
  Steamworks SDK notices.
- The `steam.exe` helper derived from Proton's steam-helper uses BSD-3-Clause,
  with separately licensed Steamworks, OpenVR, and Wine headers as applicable.
- Valve client libraries, Apple GPTK/D3DMetal, and graphics translation runtimes
  retain their own terms. Nucleon's license does not grant redistribution rights
  to those components.
