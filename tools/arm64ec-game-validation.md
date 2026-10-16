# Installed-game validation — 2026-10-16

These runs used the current debug runner on `codex/arm64-fex-dx12` and diagnostic
profiles, without changing production runtime selection or Steam registration.
Native Steam was running. Screen capture failed with ScreenCaptureKit error
`-3811`, so menus, rendered pixels and gameplay could not be inspected. The user
agreed to assist with menu quits and confirmed that GoNNER showed no game window.
No completed menu quit has been confirmed.

## Results

| Game | Runtime / graphics | Observed result | Normal menu exit | Prefix cleanup |
| --- | --- | --- | --- | --- |
| DiRT Rally 2.0 | ARM64EC / DXVK, DX11 | 180-second bound; stack-limit warnings and heap-lock timeouts | Not verified | Passed |
| Devil May Cry 5 | ARM64EC / VKD3D, DX12 | Reached swapchain creation; repeated stack-limit exceptions; controlled cancellation after about 71 seconds | Not verified | Passed |
| Braid | Baseline ARM64/FEX / WineD3D | Exited with code 255 after about 13 seconds; Steam-helper stack-limit exceptions | Failed run; not verified | Passed |
| GoNNER | Baseline ARM64/FEX / WineD3D | User confirmed no game window appeared; OpenGL framebuffer error; stopped at 180-second bound | Not verified | Passed |

Braid and GoNNER are 32-bit executables. The experimental ARM64EC frontend
contract accepts only x64 guests; their baseline runs test the existing compatible
route and do not validate ARM64EC graphics.

A bounded or cancelled runner result with exit code zero means supervised cleanup
completed. It does not establish that the game exited normally. Braid's nonzero
code is preserved. DMC5's wrapper reports failure for cancellation although the
runner records a controlled cancellation; its original game settings were restored.

## Evidence

The assembled experimental generation is
`target/arm64-fex/arm64ec-integrated/0e87f0e2c728faf01431`; the baseline is
`target/arm64-fex/integrated/7fd2ee63cb2cc9594fac`.

Local run receipts and Wine logs:

- DiRT: `target/arm64-fex/dx12/logs/dirt-690790-20261016-155120-681924.*`
- DMC5: `target/arm64-fex/dx12/logs/dmc5-dx12-20261016-155548-199339.*`
- Braid: `target/arm64-fex/game-validation/braid-1792180688541174000.*`
- GoNNER: `target/arm64-fex/game-validation/gonner-1792180820525505000.*`

Independent cleanup audits wait up to five seconds for the prefix's wineserver,
then check twice, one second apart, for remaining Wine/game processes carrying
that exact WINEPREFIX. No unrelated native Steam processes are terminated.
Audit results and the aggregate JSON are in
`target/arm64-fex/game-validation/`.

## Later baseline DiRT check

A subsequent user launch with the baseline runtime and matching 2560×1440
fullscreen resolution exited with primary code zero. The user confirmed the
screen sizing was corrected. Its old runtime still showed a black cursor;
the rebuilt runtime's native-arrow mapping is confirmed by a game trace, with
visual confirmation pending. See `tools/dirt-display-validation.json`.
This baseline follow-up does not change the ARM64EC results above.

## Production decision

Do not promote the ARM64EC frontend generation. These tests do not establish
successful menu exits, sustained gameplay or benchmark validity. Investigate the
runtime/Steam-helper stack exceptions and DiRT heap-lock stall, then repeat the
same games with confirmed rendered menus and natural exits. The baseline failures
also require investigation; they are not evidence that ARM64EC graphics caused
those failures.
