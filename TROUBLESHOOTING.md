# Troubleshooting

[README](README.md) · [Setup](SETUP.md) · [Architecture](ARCHITECTURE.md)

## Gather evidence

```sh
nucleon status
nucleon wine list
nucleon backends status
nucleon detect /path/to/game.exe
nucleon validate <AppID>
nucleon logs -n 100
```

`validate` inspects windows owned by processes in the game's prefix. A window
may be an error dialog; it does not prove frames or working input. It does not
capture the screen. Confirm visual output and input separately.

Default logs are under `~/Library/Application Support/nucleon`; an isolated
support directory has its own logs. Review local/private details before sharing.

## Running without progress

Enable Wine diagnostics when launching through Nucleon:

```sh
NUCLEON_WINEDEBUG=+timestamp,+loaddll,+msgbox,+dialog nucleon launch <appid>
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

## Steamworks connection

The Steamworks bridge still requires a compatible, signed-in client. Diagnose
ownership/session or ABI failures separately from graphics. A game that redirects
itself to Steam may require a game-specific launch recipe. Do not add
`steam_appid.txt` as a general workaround.

## Downloads and launch entries

Use `nucleon steam install <appid> --username <account>`, then `steam update` to
resume or validate managed downloads. Install DepotDownloader or supply its path
with `--downloader`. Native Steam's Downloads screen does not manage these files.

Use `nucleon steam show <appid>` for cached launch choices and choose
`--launch-id <id>` if several entries exist. `--exe` works without cached metadata.
Preview with `nucleon launch <appid> --dry-run` before changing a runtime recipe.

## Avoid regressions with LLM-assisted changes

1. Save profiles and exact runtime/backend/driver identities.
2. Isolate experimental prefixes/support files. Do not overwrite a working
   installation or kill unrelated Wine processes.
3. Change one component and capture startup/device errors.
4. Confirm visual output, input, ownership and Nucleon shutdown for affected games.
5. Promote only after those checks; retain a rollback configuration.

Compilation and a live process do not establish a game's correctness or the
safety of a private Steam ABI. Ask for a diff and evidence.

## Invalid configuration

Configuration errors appear in the CLI with the relevant setting or file and a
correction. Once a launch session starts, failures are also recorded in its
per-launch log directory (`nucleon logs`).

Engine names must be recognized (`auto`, `gptk`, `dxmt`, `dxvk`, `vkd3d`,
`kosmickrisp`, or `wine`). An explicit `auto` overrides a saved per-game engine.
Boolean environment settings accept `1`/`0` or `true`/`false`. Remove a bad variable
from the launch environment to restore the default.

`backend.toml` must contain a valid TOML string such as `backend = "auto"`.
Status output shows invalid backend settings; Automatic launches reject them.
For missing Wine installations, use `nucleon wine list` and `nucleon wine use
<name>`. Saved component paths can be reset with the corresponding command,
such as `nucleon dxvk clear-path` or `nucleon kosmickrisp clear-path`.
Vulkan overrides must name readable ICD JSON manifests whose driver libraries
exist. `VK_DRIVER_FILES` takes precedence over `VK_ICD_FILENAMES`; colon-separated
manifest lists are preserved in the game's environment.

## D3DMetal cache diagnostics

After a D3DMetal launch, `nucleon cache status` reports the current backend-owned
cache path, size and Unreal pipeline-cache files. Use `--json` for the full file
inventory and OS/runtime/framework context. Launch receipts record a snapshot,
refreshed after shutdown. `shader_caches` remains reserved for Nucleon-managed
DXVK/VKD3D/Mesa caches; `backend_caches` describes D3DMetal/application-owned files.
Discovery follows the backend's renderer-filename convention. File presence does
not establish cache hits or compatibility across OS/toolkit/GPU updates. The
command reads metadata and does not create, clear, relocate or restore caches.

For reproducible captures, `nucleon app run talos2 --run-seconds 60` requests a
60-second game runtime followed by normal prefix cleanup. Preparation and cleanup
add to the total elapsed time. This explicit option survives saved-app dispatch;
inherited `NUCLEON_*` launch controls are intentionally cleaned. Use the same
scene, route, upscaler and frame-generation settings for comparisons.
