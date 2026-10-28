# Roadmap

[README](README.md) · [Architecture](ARCHITECTURE.md)

Priority: reliable independent launching with an unmodified Steam client, automatic Windows architecture handling,
and preserved working configurations. These are planned tasks, not implemented
support claims.

## Standalone applications

- [x] Run x86/x64 Windows EXEs with explicit Wine/engine selection and isolated data.
- [x] Prepare standalone prefixes without Steam or bridge files.
- [x] Named bottles with pinned runtimes, configurable defaults and saved application entries.
- [x] Owned console probes for installer/game state sharing, child handoff, locking,
  cancellation and Unicode paths/arguments. These do not establish game compatibility.

## Reliability and safety

- [x] Remove client injection, instruction contracts, UI patches and repair guard.
- [x] Launch Steam games directly through Nucleon with explicit game identity.
- [x] Delegate Windows downloads to an independent authenticated downloader.
- [x] Preserve existing prefixes and back up retired integration state.
- [ ] Maintain a game/runtime/backend matrix covering visual presentation, input,
  ownership, shutdown, DRM relaunches and client updates.
- [ ] Validate Cloud, controller and overlay behavior outside Steam's launcher.
- [ ] Record source revisions, hashes and required source for bridge components.

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

## Steam game management

- [ ] Improve launch-entry discovery and game-specific ownership diagnostics.
- [ ] Add richer download progress while keeping authentication outside Nucleon.
- [ ] Test resuming interrupted updates and mixed native/Windows libraries.
- [ ] Investigate controller/audio issues from reproducible game reports.

A separate GUI is deferred until the core launch and download paths are validated.
