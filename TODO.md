# Nucleon Roadmap & TODO

## 1. Host Application & GUI Consolidation (Planned Milestone)
*Note: Currently intentionally deferred to keep Nucleon lean and CLI/system-first.*

- [ ] **Native macOS Host Application (SwiftUI / Tauri)**:
  - **Game Library Browser**: Seamless Steam / offline Windows title library with artwork and per-game launch configurations.
  - **Runtime & Engine Switcher**: Visual manager to switch between Apple Game Porting Toolkit (GPTK 4 / GPTK 2) and upstream Wine-Staging.
  - **Wine Prefix Inspector**: Graphical prefix explorer with registry editor (`OpenGLSurfaceMode`, `RetinaMode`, direct input mappings).
  - **Metal Graphics Settings**: Per-app toggles for Apple Metal Performance HUD (`MTL_HUD_ENABLED`), MSync (`WINEMSYNC`), Direct3D Metal shaders, and Retina HiDPI.
  - **Process Supervisor & Diagnostics**: Real-time process tree monitor (`wineserver`, `services.exe`, `.exe`), zero-overhead CoreGraphics frame telemetry, and crash dump analysis.

## 2. Compatibility & Engine Enhancements
- [ ] **String Anchor + ADRP XREF Signature Resolver**:
  - Upgrade the binary signature engine from raw Array of Bytes (AOB) to a hybrid String Anchor + ADRP Cross-Reference (XREF) analyzer.
  - Locate invariant log/debug strings in `__cstring` and trace ARM64 `ADRP` + `ADD`/`LDR` instruction pairs referencing their addresses, then backtrack to function entry prologues (`STP X29, X30, [SP, #-...]!`).
  - Provides compiler-agnostic resilience against aggressive basic block reordering, inlining, and register allocation fluctuations across major Xcode Clang updates without breaking pattern matching.
- [ ] **Dynamic WebUI Hook Updates**: Automated signature adaptation for Steam CEF chunk updates without requiring manual signature regeneration.
- [ ] **Prefix Isolation Modes**: Optional per-AppID isolated Wine prefixes (`WINEPREFIX=.../<appid>`) with shared core runtime caches.
- [ ] **Audio Latency Optimization**: CoreAudio HAL low-latency driver bridging for synchronized game audio.
- [ ] **Controller Mapping Bridge**: Enhanced SDL2 / DualSense / Xbox controller haptic feedback mapping through macOS IOHIDFamily.
