#!/usr/bin/env python3
"""Launch installed DiRT Rally 2.0 through Nucleon's normal ARM64/FEX runner.

Requires native Steam already running, the built runtime, and the local bridge.
The diagnostic profile and compatdata are separate from production game choices.
"""
import argparse
from datetime import datetime
from pathlib import Path
from arm64_fex import REPO, sha, write_json
from arm64_fex_dx12 import DX12
from nucleon_fex import installed_runtime, game_identity, launch


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--work',type=Path,default=REPO / 'target/arm64-fex')
    p.add_argument('--sdk',type=Path,default=Path.home() / 'VulkanSDK/1.4.363.0/macOS')
    p.add_argument('--runtime',type=Path)
    p.add_argument('--game-dir',type=Path,default=Path.home() /
                   'Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0')
    p.add_argument('--bridge',type=Path,default=Path.home() / 'Library/Application Support/nucleon/bridge')
    p.add_argument('--timeout',type=int)
    p.add_argument('--debug',action='store_true')
    a=p.parse_args()
    if a.timeout is not None and a.timeout < 1: p.error('--timeout must be positive')
    exe=a.game_dir.resolve() / 'dirtrally2.exe'
    if not exe.is_file(): p.error('DiRT Rally 2.0 is not installed in --game-dir')
    e=DX12(a.work.resolve(),a.sdk,6)
    runtime=a.runtime.resolve() if a.runtime else (installed_runtime() or e.prepare_runtime())
    stamp=datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    log=e.logs / f'dirt-690790-{stamp}.log'
    receipt=dict(appid=690790,runtime=str(runtime),game_identity=game_identity(exe),game_sha256=sha(exe) if a.debug else None,status='PREPARING',
                 gameplay_validation='NOT_CONFIRMED',production_profile_changed=False,runner_receipt=str(log.with_suffix('.runner.json')),log=str(log))
    try:
        receipt.update(launch(runtime,a.bridge,690790,exe,e.base.workspace / 'integrated-compatdata/690790',
            dict(graphics_api='dx11',steam_protocol='bridge',use_steam_shim=True,overlay=False),log,a.timeout,a.debug))
    except BaseException as error:
        receipt.update(status='FAILED',error=str(error))
        raise
    finally:
        write_json(log.with_suffix('.json'),receipt)


if __name__ == '__main__':
    main()
