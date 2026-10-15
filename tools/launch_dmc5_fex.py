#!/usr/bin/env python3
"""Run installed DMC5 through the isolated ARM64/FEX Vulkan/DX12 prototype.

Native Steam must be running and signed in. The game's INI is backed up and
restored on exit; Steam's real user saves remain in use. No Nucleon default or
production prefix is changed. The patched driver retains documented limitations.
"""
import argparse
from datetime import datetime
import fcntl
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time

from arm64_fex import REPO, output, run, sha, write_json
from arm64_fex_dx12 import DX12, atomic_copy, windows_path


def replace_bytes(path, data):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    try:
        temporary.chmod(path.stat().st_mode & 0o777)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def interrupted(_signum, _frame):
    raise KeyboardInterrupt


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--work', type=Path, default=REPO / 'target/arm64-fex')
    p.add_argument('--sdk', type=Path, default=Path.home() / 'VulkanSDK/1.4.363.0/macOS')
    p.add_argument('--game-dir', type=Path, default=Path.home() /
                   'Library/Application Support/Steam/steamapps/common/Devil May Cry 5')
    p.add_argument('--bridge', type=Path, default=Path.home() / 'Library/Application Support/nucleon/bridge')
    p.add_argument('--api', choices=['dx12','dx11'], default='dx12', help='DX11 is an independent Steam/CPU control')
    p.add_argument('--debug', action='store_true', help='Include Windows dialog, exception and process diagnostics')
    p.add_argument('--timeout', type=int, help='Optional bounded investigation run in seconds')
    a = p.parse_args()
    if a.timeout is not None and a.timeout < 1:
        p.error('--timeout must be positive')
    e = DX12(a.work.resolve(), a.sdk, 6)
    game, bridge = a.game_dir.resolve(), a.bridge.resolve()
    exe, ini = game / 'DevilMayCry5.exe', game / 'dmc5config.ini'
    if not exe.is_file() or not ini.is_file():
        p.error('DMC5 executable and configuration must exist in --game-dir')
    if output(['lipo','-archs',bridge / 'aarch64-unix/lsteamclient.so']) != 'arm64':
        p.error('An ARM64 Unix Steam bridge is required')
    signal.signal(signal.SIGTERM, interrupted)
    lock = (e.work / 'dmc5-launch.lock').open('a')
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        p.error('DMC5 prototype is already running')
    stamp = datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    log = e.logs / f'dmc5-{a.api}-{stamp}.log'
    report = log.with_suffix('.json')
    receipt = dict(status='PREPARING',appid=601150,api=a.api,log=str(log),
                   game_sha256=sha(exe),production_profile_changed=False,
                   gameplay_validation='NOT_CONFIRMED',runtime_limit_seconds=a.timeout)
    write_json(report,receipt)
    env = None
    original = ini.read_bytes()
    backup = e.logs / f'dmc5-settings-{stamp}.ini'
    backup.write_bytes(original)
    changed = False
    started = None
    try:
        loader, env = e.stage_prefix('dmc5-' + a.api)
        prefix = Path(env['WINEPREFIX'])
        native, steam = e.stage_steam(loader,env,bridge,log)
        if a.debug:
            env['WINEDEBUG'] = '+timestamp,+loaddll,+msgbox,+seh,+process,+msg'
            env['NUCLEON_TRACE_PROCESS_EXIT'] = '1'
        if a.api == 'dx11':
            stage = e.base.work / 'dxmt'
            system = prefix / 'drive_c/windows/system32'
            for name in ['d3d11','dxgi','d3d10core','winemetal']:
                atomic_copy(stage / f'x86_64-windows/{name}.dll',system / f'{name}.dll')
            env['WINEDLLPATH'] = str(stage) + ':' + env['WINEDLLPATH']
            env['DYLD_LIBRARY_PATH'] = str(stage / 'aarch64-unix') + ':' + env['DYLD_LIBRARY_PATH']
            env['WINEDLLOVERRIDES'] += ';d3d11,dxgi,d3d10core,winemetal=n,b'
        for user in (prefix / 'drive_c/users').iterdir():
            if user.is_dir():
                for name in ['Desktop','Documents','Downloads','Music','Pictures','Videos']:
                    folder = user / name
                    if folder.is_symlink():
                        folder.unlink(); folder.mkdir()
        platform = 'DirectX12' if a.api == 'dx12' else 'DirectX11'
        settings = original.decode('utf-8')
        for key,value in [('Capability',platform),('TargetPlatform',platform),('PCTargetAPIFallback','Disable')]:
            settings,count = re.subn(r'(?m)^'+key+r'=[^\r\n]*',key+'='+value,settings)
            if count != 1:
                raise ValueError('Expected one configuration key: ' + key)
        replace_bytes(ini,settings.encode('utf-8'))
        changed = True
        receipt.update(status='LAUNCHING',prefix=str(prefix),settings_backup=str(backup),
                       driver_sha256=sha(e.work / 'driver/libvulkan_kosmickrisp.dylib'),
                       steam_valve_sha256=sha(steam / 'steamclient64.dll'),
                       steam_pe_bridge_sha256=sha(steam / 'lsteamclient.dll'),steam_unix_sha256=sha(native),
                       steam_helper_sha256=sha(steam / 'steam.exe'))
        write_json(report,receipt)
        print(f'Launching DMC5 {a.api.upper()} with native ARM64 Wine/FEX; log: {log}',flush=True)
        started = time.monotonic()
        try:
            run([loader,steam / 'steam.exe',windows_path(exe)],cwd=game,env=env,log=log,timeout=a.timeout)
        except subprocess.TimeoutExpired:
            receipt['status'] = 'BOUNDED_RUN_FINISHED'
        else:
            receipt['status'] = 'PROCESS_EXITED_ZERO'
    except BaseException as error:
        receipt.update(status='FAILED',error=str(error))
        raise
    finally:
        if env:
            e.wine_experiment().stop_server(env)
        if changed:
            replace_bytes(ini,original)
        text = log.read_text(errors='replace') if log.exists() else ''
        receipt['steam_exports_redirected'] = 'Nucleon redirected' in text
        receipt['dx12_swapchain_created'] = 'dxgi_vk_swap_chain_init: Creating swapchain' in text
        receipt['settings_restored'] = ini.read_bytes() == original
        if started is not None:
            receipt['run_seconds'] = round(time.monotonic() - started, 3)
        write_json(report,receipt)
        lock.close()


if __name__ == '__main__':
    main()
