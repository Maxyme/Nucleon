#!/usr/bin/env python3
"""Run installed DMC5 through the isolated ARM64/FEX Vulkan/DX12 prototype.

Native Steam must be running and signed in. The game's INI is backed up and
restored on exit; Steam's real user saves remain in use. No Nucleon default or
production prefix is changed. The patched driver retains documented limitations.
"""
import argparse
from datetime import datetime
import os
from pathlib import Path
import re
import signal
import tempfile
import time

from arm64_fex import REPO, sha, write_json
from arm64_fex_dx12 import DX12
from nucleon_fex import installed_runtime, game_identity, launch, configuration_lock


def replace_bytes(path, data):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.chmod(path.stat().st_mode & 0o777)
        temporary.replace(path)
    finally:
        if temporary is not None: temporary.unlink(missing_ok=True)


def interrupted(_signum, _frame):
    raise KeyboardInterrupt


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--work', type=Path, default=REPO / 'target/arm64-fex')
    p.add_argument('--sdk', type=Path, default=Path.home() / 'VulkanSDK/1.4.363.0/macOS')
    p.add_argument('--runtime', type=Path, help='An existing complete descriptor-backed native runtime')
    p.add_argument('--game-dir', type=Path, default=Path.home() /
                   'Library/Application Support/Steam/steamapps/common/Devil May Cry 5')
    p.add_argument('--bridge', type=Path, default=Path.home() / 'Library/Application Support/nucleon/bridge')
    p.add_argument('--api', choices=['dx12','dx11'], default='dx12')
    p.add_argument('--debug', action='store_true')
    p.add_argument('--timeout', type=int, help='Game runtime limit; prefix preparation is excluded')
    a = p.parse_args()
    if a.timeout is not None and a.timeout < 1: p.error('--timeout must be positive')
    e = DX12(a.work.resolve(),a.sdk,6)
    game = a.game_dir.resolve()
    exe, ini = game / 'DevilMayCry5.exe', game / 'dmc5config.ini'
    if not exe.is_file() or not ini.is_file(): p.error('DMC5 executable and configuration are required')
    runtime = a.runtime.resolve() if a.runtime else (installed_runtime() or e.prepare_runtime())
    signal.signal(signal.SIGTERM,interrupted)
    stamp = datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    log = e.logs / f'dmc5-{a.api}-{stamp}.log'
    report = log.with_suffix('.json')
    with configuration_lock(ini):
        original = ini.read_bytes()
        backup = e.logs / f'dmc5-settings-{stamp}.ini'
        backup.write_bytes(original)
        receipt = dict(status='PREPARING',appid=601150,api=a.api,log=str(log),
                       runtime=str(runtime),settings_backup=str(backup),game_identity=game_identity(exe),game_sha256=sha(exe) if a.debug else None,
                       production_profile_changed=False,gameplay_validation='NOT_CONFIRMED',runtime_limit_seconds=a.timeout)
        write_json(report,receipt)
        changed = False
        started = time.monotonic()
        try:
            platform = 'DirectX12' if a.api == 'dx12' else 'DirectX11'
            settings = original.decode('utf-8')
            for key,value in [('Capability',platform),('TargetPlatform',platform),('PCTargetAPIFallback','Disable')]:
                settings,count = re.subn(r'(?m)^'+key+r'=[^\r\n]*',key+'='+value,settings)
                if count != 1: raise ValueError('Expected one configuration key: ' + key)
            changed = True
            replace_bytes(ini,settings.encode('utf-8'))
            receipt.update(status='LAUNCHING')
            write_json(report,receipt)
            print(f'Launching DMC5 {a.api.upper()} through nucleon-runner; log: {log}',flush=True)
            receipt.update(launch(runtime,a.bridge,601150,exe,e.base.workspace / 'integrated-compatdata/601150',
                dict(engine='vkd3d' if a.api=='dx12' else 'dxmt',graphics_api=a.api,
                     steam_protocol='valve-redirect',use_steam_shim=True,overlay=False),log,a.timeout,a.debug))
        except BaseException as error:
            receipt.update(status='FAILED',error=str(error))
            raise
        finally:
            if changed: replace_bytes(ini,original)
            wine_log = log.with_suffix('.wine.log')
            text = wine_log.read_text(errors='replace') if wine_log.exists() else ''
            receipt['steam_exports_redirected'] = 'Nucleon redirected' in text
            receipt['dx12_swapchain_created'] = 'dxgi_vk_swap_chain_init: Creating swapchain' in text
            receipt['settings_restored'] = ini.read_bytes() == original
            receipt['elapsed_seconds_including_preparation'] = round(time.monotonic()-started,3)
            write_json(report,receipt)


if __name__ == '__main__':
    main()
