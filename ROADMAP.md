# Roadmap

[README](README.md) · [Architecture](ARCHITECTURE.md)

Priority: a reliable native Steam flow, automatic Windows architecture handling,
and preserved working configurations. These are planned tasks, not implemented
support claims.

## Standalone applications

- [x] Run x86/x64 Windows EXEs with explicit Wine/engine selection and isolated data.
- [x] Prepare standalone prefixes without Steam or bridge files.
- [x] Named bottles with pinned runtimes, configurable defaults and saved application entries.
- [x] Owned console probes for installer/game state sharing, child handoff, locking,
  cancellation and Unicode paths/arguments. These do not establish game compatibility.

## Reliability and safety

- [ ] Recheck GoNNER and supported GPTK pairings without the copied overlay.
- [ ] Validate the compatibility contract's new patch strategy in live Steam.
- [ ] Replace remaining spawn/filesystem trampoline globals and prove publication.
- [ ] Set Steam's tool environment before process startup.
- [ ] Extend reviewed client coverage and validate callback ABI/image lifetimes;
  retain fail-closed behavior on unsupported clients.
- [ ] Maintain a Steam/runtime/backend regression matrix covering presentation,
  input, ownership, and shutdown.
- [ ] Record source revisions, hashes, licenses, and required source for
  distributed bridge/runtime components.

## Runtime and graphics

- [ ] Prefer obtainable Wine with a small documented patch set; pursue upstream
  fixes for Rosetta and loader behavior where suitable.
- [ ] Validate an architecture-compatible Vulkan driver before promoting
  KosmicKrisp as a general default for x86_64 Wine.
- [ ] Validate DX12/VKD3D-Proton, DXMT, and D7VK recipes per game.
- [ ] Research Zink and ARM/FEX execution without presenting them as automatic
  replacements for confirmed stacks.
- [ ] Independently implement microphone/GPTK workarounds only for demonstrated
  needs with clear runtime contracts.

## Steam integration

- [ ] Improve signature recovery using string anchors and instruction references,
  retaining build gates and refusing ambiguity.
- [ ] Improve missing-bridge, ABI, driver-architecture, and session diagnostics.
- [ ] Make configuration and rollback easier within the Steam flow.
- [ ] Investigate controller/audio issues from reproducible game reports.

A separate GUI, broad game-library browser, and automated generated patches are
deferred until the reliability and safety work is addressed.
