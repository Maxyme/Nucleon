"""Diagnostic entry points into Nucleon's normal ARM64/FEX launch flow."""
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import tomllib

from arm64_fex import REPO, write_json


@contextmanager
def configuration_lock(path):
    # The actual config path may be shared by different prefixes/work directories.
    key = hashlib.sha256(str(path.resolve()).encode()).hexdigest()
    lock = Path(tempfile.gettempdir()) / f'nucleon-config-{os.getuid()}-{key}.lock'
    with lock.open('a') as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError(f'Another diagnostic launch is using {path}') from error
        yield
    # Leave the inode in place so contenders cannot lock different files.


def interrupted(_signum, _frame):
    raise KeyboardInterrupt


def installed_runtime():
    """Prefer the pinned managed generation; no extraction, SDK or compiler needed."""
    selection = Path.home() / 'Library/Application Support/Steam/compatibilitytools.d/nucleon-fex/fex-runtime.json'
    if not selection.is_file(): return None
    selected = json.loads(selection.read_text())
    root = Path(selected['root'])
    if selected.get('managed') and not (root / '.nucleon-install.json').is_file():
        raise ValueError('Installed FEX runtime is incomplete; run nucleon wine fex verify')
    if (root / '.nucleon-install.json').is_file():
        if not (root / 'nucleon-runtime.toml').is_file():
            raise ValueError('Installed FEX runtime is incomplete; run nucleon wine fex verify')
        return root.resolve()
    return None


def game_identity(path):
    value = path.stat()
    return dict(size=value.st_size, modified_ns=value.st_mtime_ns,
                changed_ns=value.st_ctime_ns, device=value.st_dev, inode=value.st_ino)


def launch(runtime, bridge, appid, executable, compat_data, profile, log, timeout=None, debug=False):
    runtime = runtime.resolve()
    descriptor = tomllib.loads((runtime / 'nucleon-runtime.toml').read_text())
    runner = Path(os.environ.get('NUCLEON_RUNNER', REPO / 'target/debug/nucleon-runner'))
    if not runner.is_file():
        raise ValueError('Build the normal runner first: cargo build -p nucleon-runner')
    env = {k:v for k,v in os.environ.items()
           if not k.startswith(('WINE','DYLD_','FEX_','MACRUNNER_','NUCLEON_'))
           and k not in ('VK_DRIVER_FILES','VK_ICD_FILENAMES','VKD3D_CONFIG',
                         'VKD3D_FEATURE_LEVEL','VKD3D_SHADER_MODEL','MESA_VK_VERSION_OVERRIDE',
                         'MVK_CONFIG_ADVERTISE_API_VERSION')}
    if 'NUCLEON_SUPPORT_DIR' in os.environ:
        env['NUCLEON_SUPPORT_DIR'] = os.environ['NUCLEON_SUPPORT_DIR']
    wine_log = log.with_suffix('.wine.log')
    prefix = compat_data / 'nucleon/arm64-fex' / descriptor['prefix']['family'] / 'pfx'
    with tempfile.TemporaryDirectory(prefix='nucleon-fex-launch-') as temporary:
        temporary = Path(temporary)
        selection = temporary / 'runtime.json'
        write_json(selection,dict(schema=1,root=str(runtime),prefix=descriptor['prefix']))
        profile_file = temporary / 'profile.json'
        write_json(profile_file,profile)
        env.update(NUCLEON_FEX_SELECTION=str(selection),NUCLEON_GAME_PROFILE=str(profile_file),
                   NUCLEON_STEAM_BRIDGE=str(bridge.resolve()),NUCLEON_WINE_LOG=str(wine_log),
                   STEAM_COMPAT_APP_ID=str(appid),STEAM_COMPAT_DATA_PATH=str(compat_data.resolve()),
                   SteamAppId=str(appid),SteamGameId=str(appid))
        if timeout: env['NUCLEON_RUN_SECONDS'] = str(timeout)
        if debug: env.update(NUCLEON_WINEDEBUG='+timestamp,+loaddll,+msgbox,+seh,+process,+msg',NUCLEON_TRACE_PROCESS_EXIT='1')
        with log.open('ab') as stream:
            process = subprocess.Popen([str(runner),'run',str(executable)],cwd=executable.parent,
                env=env,stdout=stream,stderr=subprocess.STDOUT,start_new_session=True)
            previous_handler = signal.signal(signal.SIGTERM, interrupted)
            try:
                code = process.wait()
            except BaseException:
                # The Rust owner needs time to stop its separate Wine process
                # group and wineserver before this diagnostic wrapper is reaped.
                try: process.send_signal(signal.SIGTERM)
                except ProcessLookupError: pass
                try: process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                raise
            finally:
                signal.signal(signal.SIGTERM, previous_handler)
        if code:
            raise RuntimeError(f'Nucleon exited {code}; see {log} and {wine_log}')
    text = log.read_text(errors='replace')
    return dict(prefix=str(prefix),wine_log=str(wine_log),profile=profile,
                status='BOUNDED_RUN_FINISHED' if 'Bounded run finished;' in text else 'PROCESS_EXITED_ZERO')
