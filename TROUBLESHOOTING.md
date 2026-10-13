# Troubleshooting

[README](README.md) · [Setup](SETUP.md) · [Architecture](ARCHITECTURE.md)

## Gather evidence

```sh
nucleon status
nucleon wine list
nucleon backends status
nucleon detect /path/to/game.exe
nucleon validate <AppID>
nucleon logs --runner -n 100
nucleon logs --hook -n 100
```

`validate` inspects windows owned by processes in the game's prefix. A window
may be an error dialog; it does not prove frames or working input. It does not
capture the screen. Confirm visual output and input separately.

Default logs are under `~/Library/Application Support/nucleon`; an isolated
support directory has its own logs. Review local/private details before sharing.

## Running without progress

Set the game's Steam launch options:

```sh
NUCLEON_WINEDEBUG=+timestamp,+loaddll,+msgbox,+dialog %command%
```

Check `wine.log` for DLL initialization errors, message-box text, Steam pipe
failures, and where graphics initialization stops. A modal dialog can look like
a graphics hang. Diagnose ownership with a live native Steam client session.

## Steamworks and Denuvo

For `CreateSteamPipe failed` or `Application load error 3:0000065432`, inspect
the account session, AppID, prefix, client DLL provenance, helper selection,
and bridge/ntdll pairing before changing graphics. Adding `steam_appid.txt` was
not a valid DMC5 ownership fix: it changed the launch mode.

`NUCLEON_STEAM_SHIM=0|1` chooses direct execution or the prefix helper in an
experiment. DMC5's confirmed recipe also uses a private loader-patched Wine:
Valve's original PE stays on disk while mapped exports route to `lsteamclient`.

## Frozen window or device failure

Record actual Wine, backend DLLs, driver ICD, and feature level. Change one
component at a time. Device-loss and Metal invalid-resource errors need more
investigation than checking that a window exists.

DiRT's newer DXVK experiment froze on GPU submissions. Its working baseline
uses pinned DXVK-macOS 1.10.3/MoltenVK. DMC5's baseline uses DX11. Neither proves
that newer DXVK or DX12 works. ARM-only KosmicKrisp cannot load into x86_64 Wine;
check architecture and feature requirements before shader/device debugging.

## Steam update or overlay changes

```sh
nucleon guard status
nucleon guard run
nucleon logs --hook -n 100
```

Restart Steam after repair. A repaired UI does not prove private ABI offsets
or hook publication safety. See [remaining risks](ARCHITECTURE.md#safety-boundaries).

The copied overlay shim is absent from new builds. Opt-in uses Valve's native
renderer only. HDR/Metal 4 corrections, microphone suppression, and the old
experimental Wine bridge are absent. GoNNER and GPTK paths need another check;
older installed binaries may still contain the shim until deliberately updated.

## Avoid regressions with LLM-assisted changes

1. Save profiles and exact runtime/backend/driver identities.
2. Isolate experimental prefixes/support files. Do not overwrite a working
   installation or kill unrelated Wine processes.
3. Change one component and capture startup/device errors.
4. Confirm visual output, input, ownership, and Steam Stop for affected games.
5. Promote only after those checks; retain a rollback configuration.

Compilation and a live process do not establish a game's correctness or the
safety of a private Steam ABI. Ask for a diff and evidence.

## Records

- [Runtime recovery](docs/game-runtime-recovery.md)
- [DMC5 investigation](docs/dmc5-native-steam-investigation.md)
- [Direct ntdll binding](docs/ntdll-compat-rust-investigation.md)
- [Unsafe audit](docs/unsafe-audit-2026-10-13.md)
- [Provenance audit](docs/notproton-provenance-and-overlay-audit.md)
