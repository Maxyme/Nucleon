#!/usr/bin/env python3
"""Launch the installed DiRT Rally 2.0 with the isolated Tolkara experiment.

Requires the built runtime, staged DXMT smoke-test components, Nucleon's local
Steam bridge and a running, signed-in native Steam client. No runtime selection
or installed game files are changed. Steam saves still use the real user data.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile
from datetime import datetime

from arm64_fex import Experiment, REPO, output, run, sha, write_json


def atomic_copy(source, destination):
    with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as stream:
        temporary = Path(stream.name)
    try:
        shutil.copy2(source, temporary)
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, default=REPO / 'target/arm64-fex')
    parser.add_argument('--game-dir', type=Path, default=Path.home() /
                        'Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0')
    parser.add_argument('--bridge', type=Path, default=Path.home() /
                        'Library/Application Support/nucleon/bridge')
    args = parser.parse_args()
    game, bridge = args.game_dir.resolve(), args.bridge.resolve()
    executable = game / 'dirtrally2.exe'
    if not executable.is_file():
        parser.error('DiRT Rally 2.0 is not installed at --game-dir')
    e = Experiment(args.work.resolve(), 6, 'tolkara')
    stage = e.work / 'dxmt'
    native = bridge / 'aarch64-unix/lsteamclient.so'
    if output(['lipo', '-archs', native]) != 'arm64':
        parser.error('Steam bridge must have an ARM64 Unix companion')
    run(['codesign', '--verify', '--strict', stage / 'aarch64-unix/winemetal.so'])
    loader, env = e.runtime_environment('dirt-690790')
    timestamp = datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    log = e.logs / f'dirt-690790-{timestamp}.log'
    receipt = dict(appid=690790, variant='tolkara', game_sha256=sha(executable),
                   steam_pe_sha256=sha(bridge / 'x86_64-windows/lsteamclient.dll'),
                   steam_unix_source_sha256=sha(native), log=str(log), status='PREPARING')
    report = e.logs / f'dirt-690790-{timestamp}.json'
    write_json(report, receipt)
    try:
        run([loader, 'wineboot', '-i'], env=env, log=log, timeout=120)
        prefix = Path(env['WINEPREFIX'])
        # Keep ordinary Windows document settings in the experimental prefix.
        for user in (prefix / 'drive_c/users').iterdir():
            if not user.is_dir():
                continue
            for name in ['Desktop', 'Documents', 'Downloads', 'Music', 'Pictures', 'Videos']:
                folder = user / name
                if folder.is_symlink():
                    folder.unlink()
                    folder.mkdir()
        system = prefix / 'drive_c/windows/system32'
        for name in ['d3d11', 'dxgi', 'd3d10core', 'winemetal']:
            atomic_copy(stage / f'x86_64-windows/{name}.dll', system / f'{name}.dll')
        steam = prefix / 'drive_c/Program Files (x86)/Steam'
        steam.mkdir(parents=True, exist_ok=True)
        for name in ['lsteamclient', 'steamclient64']:
            for destination in [steam, system]:
                atomic_copy(bridge / 'x86_64-windows/lsteamclient.dll', destination / f'{name}.dll')
            target = steam / f'{name}.so'
            atomic_copy(native, target)
            run(['install_name_tool', '-change', '@rpath/ntdll.so', '@loader_path/ntdll.so', target])
            run(['codesign', '--force', '--sign', '-', '--timestamp=none', target])
            run(['codesign', '--verify', '--strict', target])
        ntdll = steam / 'ntdll.so'
        ntdll.unlink(missing_ok=True)
        ntdll.symlink_to(e.work / 'runtime/lib/wine/aarch64-unix/ntdll.so')
        atomic_copy(bridge / 'steam.exe', steam / 'steam.exe')
        env['STEAM_COMPAT_CLIENT_INSTALL_PATH'] = str(Path.home() /
            'Library/Application Support/Steam/Steam.AppBundle/Steam/Contents/MacOS')
        env['WINEDLLPATH'] = f'{steam}:{stage}:' + env['WINEDLLPATH']
        env['DYLD_LIBRARY_PATH'] = f'{stage}/aarch64-unix:' + env['DYLD_LIBRARY_PATH']
        env['WINEDLLOVERRIDES'] += ';d3d11,dxgi,d3d10core,winemetal=n,b;steamclient64=n,b;lsteamclient=n,b'
        env.update(SteamAppId='690790', SteamGameId='690790',
                   WINEDEBUG='+loaddll', WINEMETAL_TRACE_HWND='1')
        receipt.update(status='LAUNCHING', prefix=str(prefix),
                       steam_unix_staged_sha256=sha(steam / 'lsteamclient.so'))
        write_json(report, receipt)
        print(f'Launching DiRT Rally 2.0; log: {log}', flush=True)
        run([loader, steam / 'steam.exe', executable], cwd=game, env=env, log=log)
        receipt['status'] = 'PROCESS_EXITED_ZERO'
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        receipt.update(status='FAILED', error=str(error))
        raise
    finally:
        e.stop_server(env)
        write_json(report, receipt)


if __name__ == '__main__':
    main()
