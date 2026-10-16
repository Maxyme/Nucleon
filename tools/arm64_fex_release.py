#!/usr/bin/env python3
"""Build and package the source-built, experimental ARM64 Wine/FEX core.

No reference-app binaries, Steam libraries, SDK binaries or graphics add-ons are
included. Packaging always validates a relocated install and both guest ABIs.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

from arm64_fex import Experiment, RECIPE, REPO, apply_wine_patches, atomic_copy, output, run, sha, write_json

TAG_PREFIX = 'runtime-arm64-fex-v'
RELEASE_FILES = ['tools/arm64_fex_release.py', 'tools/test_arm64_fex_release.py',
                 '.github/workflows/arm64-fex-runtime.yml', 'RUNTIME-RELEASES.md']


def mark_changes(root, paths, version, date):
    """Keep prominent modification/date notices in the actual compiled sources."""
    for relative in paths:
        path = root / relative
        prefix = '# ' if path.suffix == '.spec' else '/* '
        end = '' if path.suffix == '.spec' else ' */'
        if path.suffix not in ('.c', '.cpp', '.h', '.m', '.spec'):
            raise ValueError('Unsupported modified-source notice format: ' + relative)
        text = path.read_text()
        if text.startswith(prefix + 'Nucleon runtime downstream modifications:'):
            text = text.split('\n', 1)[1]
        notice = f'{prefix}Nucleon runtime downstream modifications: {date}, runtime {version}.{end}\n'
        changed = notice + text
        if path.read_text() != changed:
            path.write_text(changed)


def prepare_publisher_sources(experiment, version):
    settings = experiment.recipe['wine']['tolkara']
    helper = settings['shared_data_helper']
    original = experiment.sources / 'wine' / helper
    if sha(original) != settings['shared_data_helper_sha256']:
        raise ValueError('Shared-data source checksum differs')
    atomic_copy(original, experiment.wine / helper)
    run(['git', '-C', experiment.wine, 'add', '--intent-to-add', helper])
    date = output(['git', '-C', REPO, 'show', '-s', '--format=%cs', 'HEAD'])
    for root, patches in [(experiment.wine, settings['patches']),
                           (experiment.fex, experiment.recipe['alternatives']['tolkara']['fex']['unix_patches'])]:
        apply_wine_patches(root, patches)
        changed = output(['git', '-C', root, 'diff', '--name-only', 'HEAD']).splitlines()
        # The borrowed header is copied unchanged, with its original Wine notices.
        mark_changes(root, [name for name in changed if name != helper], version, date)
        apply_wine_patches(root, patches)  # Record the complete downstream source overlay.


def version_name(value):
    version = value.removeprefix(TAG_PREFIX)
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?', version):
        raise ValueError('Expected a runtime version such as 0.1.0-rc.1')
    return version


def source_files(root):
    """Export tracked working-tree contents, including initialized submodules.

    In particular, export patched bytes, not git archive HEAD. Never sweep up
    ignored build products or arbitrary untracked publisher files.
    """
    entries = subprocess.check_output(['git', '-C', str(root), 'ls-files', '--stage', '-z'])
    for entry in entries.split(b'\0'):
        if not entry:
            continue
        metadata, raw_name = entry.split(b'\t', 1)
        mode, oid, stage = metadata.split()
        name = os.fsdecode(raw_name)
        if stage != b'0':
            raise ValueError('Unmerged source path: ' + name)
        path = root / name
        if mode == b'160000':
            if (path / '.git').exists():
                if output(['git', '-C', path, 'rev-parse', 'HEAD']) != oid.decode():
                    raise ValueError('Submodule revision differs from its source pin: ' + name)
                for child in source_files(path):
                    yield Path(name) / child
        else:
            if not path.is_file() and not path.is_symlink():
                raise ValueError('Missing source path: ' + str(path))
            yield Path(name)


def checked_link(root, path):
    target = os.readlink(path)
    if os.path.isabs(target) or not path.resolve(strict=True).is_relative_to(root.resolve()):
        raise ValueError('Symlink escapes package: ' + str(path))
    return target


def archive_files(destination, entries, epoch):
    """Write stable archives, with no host ownership or absolute file names."""
    with destination.open('xb') as raw:
        with gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=epoch, compresslevel=6) as zipped:
            with tarfile.open(fileobj=zipped, mode='w', format=tarfile.PAX_FORMAT) as archive:
                for root, relative, archive_name in sorted(entries, key=lambda e: e[2]):
                    path = root / relative
                    info = archive.gettarinfo(str(path), arcname=archive_name)
                    if not (info.isfile() or info.issym()):
                        raise ValueError('Unsupported archive entry: ' + str(path))
                    if info.issym():
                        checked_link(root, path)
                    if info.mode & 0o6000:
                        raise ValueError('Privileged archive entry: ' + str(path))
                    info.uid = info.gid = 0
                    info.uname = info.gname = ''
                    info.mtime = epoch
                    info.pax_headers = {}
                    if info.isfile():
                        with path.open('rb') as stream:
                            archive.addfile(info, stream)
                    else:
                        archive.addfile(info)


def tree_entries(root, prefix=''):
    for path in sorted(root.rglob('*')):
        if path.is_symlink() or path.is_file():
            relative = path.relative_to(root)
            yield root, relative, prefix + relative.as_posix()


def core_descriptor():
    return '''schema = 1
mode = "arm64-fex"
guests = ["x64", "x86"]
loader = "bin/wine"
server = "bin/wineserver"
unix_dir = "lib/wine/aarch64-unix"
windows_dirs = ["lib/wine/aarch64-windows", "lib/wine/i386-windows"]
backends = ["auto"]
steam_helper = true
steam_protocol = "bridge"
[prefix]
family = "tolkara-fex"
abi = 1
[[emulators]]
guest = "x64"
path = "lib/wine/aarch64-windows/libarm64ecfex.dll"
unix_library = "lib/wine/aarch64-unix/libarm64ecfex.so"
[[emulators]]
guest = "x86"
path = "lib/wine/aarch64-windows/libwow64fex.dll"
unix_library = "lib/wine/aarch64-unix/libwow64fex.so"
[environment]
FEX_TSOENABLED = "1"
NUCLEON_EMULATE_KUSD = "1"
'''


def build_identity(experiment, source_manifest):
    runtime = experiment.work / 'runtime'
    files = {}
    for name in ['bin/wine', 'bin/wineserver', 'lib/wine', 'share/wine',
                 'share/licenses', 'share/nucleon-build', 'build.json']:
        path = runtime / name
        paths = path.rglob('*') if path.is_dir() else [path]
        for item in paths:
            if item.suffix == '.a':
                continue
            if item.is_symlink():
                files[item.relative_to(runtime).as_posix()] = {'link': checked_link(runtime, item)}
            elif item.is_file():
                files[item.relative_to(runtime).as_posix()] = dict(sha256=sha(item), mode=item.stat().st_mode & 0o777)
    for directory, name in [('arm64ec', 'libarm64ecfex.dll'), ('wow64', 'libwow64fex.dll')]:
        files['fex/' + name] = sha(experiment.work / 'fex' / directory / 'Bin' / name)
    for name in ['libarm64ecfex.so', 'libwow64fex.so']:
        files['fex/' + name] = sha(experiment.work / 'fex/unixlib' / name)
    sources = json.loads(source_manifest.read_text())
    compiled = {key: value for key, value in sources.items() if key != 'nucleon'}
    compiled['build-scripts'] = {key: value for key, value in sources['nucleon']['files'].items()
                                 if key.startswith(('assets/wine/recipes/', 'assets/patches/'))
                                 or key == 'tools/arm64_fex.py'}
    identity = hashlib.sha256(json.dumps(compiled, sort_keys=True).encode()).hexdigest()
    return dict(compiled_sources_sha256=identity, files=files)


def stage_core(experiment, destination):
    runtime = experiment.work / 'runtime'
    receipt = json.loads((runtime / 'build.json').read_text())
    expected_patches = {p: sha(REPO / p) for p in experiment.recipe['wine']['tolkara']['patches']}
    if receipt['patches'] != expected_patches or receipt['recipe_sha256'] != sha(RECIPE):
        raise ValueError('Wine build does not match the current release recipe; rebuild Wine')
    for relative, digest in receipt['host_files'].items():
        if sha(runtime / relative) != digest:
            raise ValueError('Wine build receipt mismatch: ' + relative)
    destination.mkdir()
    for name in ['wine', 'wineserver']:
        atomic_copy(runtime / 'bin' / name, destination / 'bin' / name)
    for name in ['lib/wine', 'share/wine', 'share/licenses', 'share/nucleon-build']:
        if (runtime / name).exists():
            shutil.copytree(runtime / name, destination / name, symlinks=True,
                            ignore=shutil.ignore_patterns('*.a'))
    for directory, name in [('arm64ec', 'libarm64ecfex.dll'), ('wow64', 'libwow64fex.dll')]:
        atomic_copy(experiment.work / 'fex' / directory / 'Bin' / name,
                    destination / 'lib/wine/aarch64-windows' / name)
    unix = experiment.work / 'fex/unixlib'
    unix_receipt = json.loads((unix / 'build.json').read_text())
    for name, digest in unix_receipt['files'].items():
        if sha(unix / name) != digest:
            raise ValueError('FEX companion receipt mismatch: ' + name)
        atomic_copy(unix / name, destination / 'lib/wine/aarch64-unix' / name)
    atomic_copy(runtime / 'build.json', destination / 'share/nucleon-build/wine-build.json')
    atomic_copy(unix / 'build.json', destination / 'share/nucleon-build/fex-unix-build.json')
    # Restrict packaging to this core assembler, never an assembled reference-app runtime.
    (destination / 'nucleon-runtime.toml').write_text(core_descriptor())
    atomic_copy(REPO / 'NOTICE.md', destination / 'share/nucleon-build/NOTICE.md')
    atomic_copy(REPO / 'LICENSE', destination / 'share/licenses/nucleon/LICENSE')
    atomic_copy(experiment.compiler / 'LICENSE.TXT', destination / 'share/licenses/toolchain/LLVM-LICENSE.TXT')
    for triple in ['arm64ec', 'aarch64', 'i686']:
        notices = experiment.compiler / f'{triple}-w64-mingw32/share/mingw32'
        if not notices.is_dir():
            raise ValueError('Missing MinGW runtime notices: ' + triple)
        shutil.copytree(notices, destination / 'share/licenses/toolchain' / triple)

    images, native = [], []
    for path in destination.rglob('*'):
        if path.is_symlink():
            checked_link(destination, path)
        elif path.is_file():
            with path.open('rb') as stream:
                magic = stream.read(4)
            if magic.startswith(b'MZ'):
                images.append(path)
            if magic in (b'\xcf\xfa\xed\xfe', b'\xfe\xed\xfa\xcf'):
                native.append(path)
    for offset in range(0, len(images), 128):
        run([experiment.compiler / 'bin/llvm-strip', '--strip-debug', *images[offset:offset + 128]])
    for path in native:
        run(['codesign', '--force', '--sign', '-', '--timestamp=none', path])
        run(['codesign', '--verify', '--strict', path])


def source_snapshot(experiment, stage):
    entries, receipts = [], {}
    for label, root, expected in [
        ('wine', experiment.wine, experiment.recipe['alternatives']['tolkara']['wine']['revision']),
        ('fex', experiment.fex, experiment.recipe['alternatives']['tolkara']['fex']['revision']),
        ('shared-data-origin', experiment.sources / 'wine', experiment.recipe['sources']['wine']['revision']),
        ('nucleon', REPO, None),
    ]:
        revision = output(['git', '-C', root, 'rev-parse', 'HEAD'])
        if expected and revision != expected:
            raise ValueError('Unexpected source revision: ' + label)
        files = set(source_files(root))
        if label == 'nucleon':
            files.update(Path(name) for name in RELEASE_FILES)
        digests = {}
        snapshot = stage / 'sources' / label
        for relative in sorted(files):
            path = root / relative
            copied = snapshot / relative
            copied.parent.mkdir(parents=True, exist_ok=True)
            if path.is_symlink():
                copied.symlink_to(checked_link(root, path))
                digests[relative.as_posix()] = {'link': os.readlink(copied)}
            else:
                shutil.copy2(path, copied)
                digests[relative.as_posix()] = {'sha256': sha(copied)}
            entries.append((stage, copied.relative_to(stage), 'sources/' + label + '/' + relative.as_posix()))
        for relative in files:
            if (snapshot / relative).is_symlink():
                checked_link(snapshot, snapshot / relative)
        changes = output(['git', '-C', root, 'diff', '--name-only', 'HEAD']).splitlines()
        receipts[label] = dict(revision=revision, downstream_modified_paths=changes, files=digests)
    write_json(stage / 'SOURCE-MANIFEST.json', receipts)
    entries.append((stage, Path('SOURCE-MANIFEST.json'), 'SOURCE-MANIFEST.json'))
    return entries


def validate_install(nucleon, archive, support, experiment, logs):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('WINE', 'DYLD_', 'FEX_', 'MACRUNNER_', 'NUCLEON_'))}
    env['NUCLEON_SUPPORT_DIR'] = str(support)
    run([nucleon, 'wine', 'fex', 'install', archive, '--sha256', sha(archive)], env=env,
        log=logs / 'install.log', timeout=300)
    installed = list((support / 'runtimes/arm64-fex').glob('*/.nucleon-install.json'))
    if len(installed) != 1:
        raise ValueError('Expected exactly one isolated managed installation')
    runtime = installed[0].parent
    run([nucleon, 'wine', 'fex', 'verify', runtime], env=env, log=logs / 'verify.log', timeout=300)
    descriptor = tomllib.loads((runtime / 'nucleon-runtime.toml').read_text())
    env.update(descriptor['environment'])
    env.update(WINEPREFIX=str(support / 'smoke-prefix'), WINELOADER=str(runtime / 'bin/wine'),
               WINESERVER=str(runtime / 'bin/wineserver'),
               WINEDLLPATH=':'.join(str(runtime / p) for p in descriptor['windows_dirs']),
               DYLD_LIBRARY_PATH=str(runtime / descriptor['unix_dir']),
               DYLD_FALLBACK_LIBRARY_PATH=str(runtime / 'lib'), WINEDEBUG='-all,warn+all,err+all',
               WINEDLLOVERRIDES='winemenubuilder.exe=d;mscoree=d;mshtml=d')
    results = {}
    try:
        run([runtime / 'bin/wine', 'wineboot', '-i'], env=env, log=logs / 'wineboot.log', timeout=120)
        for guest, triple, bits in [('x64', 'x86_64', 64), ('x86', 'i686', 32)]:
            exe = logs / f'smoke-{guest}.exe'
            run([experiment.compiler / 'bin' / f'{triple}-w64-mingw32-clang', '-O2', '-municode',
                 '-fms-extensions', REPO / 'tools/runtime-probes/windows.c', '-o', exe],
                env=experiment.env, log=logs / f'compile-{guest}.log')
            log = logs / f'smoke-{guest}.log'
            run([runtime / 'bin/wine', exe], env=env, log=log, timeout=60)
            text = log.read_text(errors='replace')
            if f'NUCLEON_FEX_BEGIN pointer_bits={bits}' not in text or 'NUCLEON_FEX_SMOKE_OK' not in text:
                raise ValueError('Missing guest smoke result: ' + guest)
            results[guest] = 'PASSED'
    finally:
        # This prefix is created by this invocation, never a Steam/game prefix.
        original_error = sys.exc_info()[0] is not None
        try:
            with (logs / 'cleanup.log').open('ab') as stream:
                killed = subprocess.run([str(runtime / 'bin/wineserver'), '-k'], env=env,
                                        stdout=stream, stderr=subprocess.STDOUT, timeout=10)
            if killed.returncode not in (0, 1):  # -k returns 1 when no server was started.
                raise ValueError('Cannot stop the owned smoke-test server')
            run([runtime / 'bin/wineserver', '-w'], env=env, log=logs / 'cleanup.log', timeout=10)
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            if not original_error:
                raise
            with (logs / 'cleanup.log').open('a') as stream:
                stream.write(f'Cleanup after primary failure: {error}\n')
    return dict(managed_archive_import='PASSED', integrity_and_native_dependencies='PASSED',
                guest_smoke=results, prefix_cleanup='PASSED', steam='NOT_RUN', gameplay='NOT_RUN')


def package(experiment, version, destination, nucleon):
    version = version_name(version)
    if destination.exists():
        raise ValueError('Output must not exist; preserve existing release assets')
    destination.parent.mkdir(parents=True, exist_ok=True)
    epoch = int(output(['git', '-C', REPO, 'show', '-s', '--format=%ct', 'HEAD']))
    with tempfile.TemporaryDirectory(prefix='.runtime-release-', dir=destination.parent) as temporary:
        work = Path(temporary)
        assets = work / 'assets'
        assets.mkdir()
        log_root = experiment.work / 'release-validation'
        log_root.mkdir(exist_ok=True)
        logs = Path(tempfile.mkdtemp(prefix=version + '-', dir=log_root))
        stage = work / 'runtime'
        stage_core(experiment, stage)
        sources = source_snapshot(experiment, work)
        built = json.loads((experiment.work / 'publisher-build.json').read_text())
        if built != build_identity(experiment, work / 'SOURCE-MANIFEST.json'):
            raise ValueError('Sources or products changed since publisher build; run build again')
        binary = assets / f'nucleon-arm64-fex-core-{version}-macos-arm64.tar.gz'
        source = assets / f'nucleon-arm64-fex-core-{version}-sources.tar.gz'
        manifest = dict(schema=1, version=version, channel='experimental', component='wine-fex-core',
                        postprocessing=['PE llvm-strip --strip-debug', 'Mach-O ad-hoc signing; no Mach-O stripping'],
                        guests=['x64', 'x86'], graphics='WineD3D; graphics add-ons distributed separately',
                        build_macos=output(['sw_vers', '-productVersion']),
                        toolchain=experiment.recipe['toolchain'], source_manifest_sha256=sha(work / 'SOURCE-MANIFEST.json'))
        write_json(stage / 'share/nucleon-build/release.json', manifest)
        archive_files(binary, tree_entries(stage), epoch)
        archive_files(source, sources, epoch)
        validation = validate_install(nucleon, binary, work / 'support', experiment, logs)
        manifest.update(binary=dict(file=binary.name, sha256=sha(binary)),
                        sources=dict(file=source.name, sha256=sha(source)), validation=validation)
        write_json(assets / 'runtime-release.json', manifest)
        (assets / 'RELEASE-NOTES.md').write_text(
            f'# ARM64 Wine/FEX core {version} (experimental)\n\n'
            'Prebuilt native ARM64 Wine with x64 and x86 FEX emulators and matching native companions.\n'
            'Users do not need a compiler. WineD3D is included; DXMT, DXVK/VKD3D, Vulkan drivers,\n'
            'Steam bridges/helpers, Valve libraries and Apple GPTK are not bundled.\n'
            'This CPU/runtime prerelease does not certify Steam, graphics or game compatibility.\n\n'
            'Download the runtime and SHA256SUMS.txt, then install using its published checksum:\n\n'
            f'```sh\nnucleon wine fex install ./{binary.name} --sha256 {sha(binary)} --register\n'
            'nucleon wine fex verify\n```\n\n'
            f'Corresponding patched sources, notices and build scripts: `{source.name}`.\n'
            'Rebuild instructions are in `sources/nucleon/RUNTIME-RELEASES.md`.\n')
        checksums = ''.join(f'{sha(p)}  {p.name}\n' for p in sorted(assets.iterdir()) if p.is_file())
        (assets / 'SHA256SUMS.txt').write_text(checksums)
        shutil.copytree(logs, assets / 'validation')
        assets.rename(destination)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['build', 'package'])
    parser.add_argument('--version', required=True)
    parser.add_argument('--work', type=Path, default=REPO / 'target/runtime-release')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--nucleon', type=Path, default=REPO / 'target/release/nucleon')
    parser.add_argument('--jobs', type=int, default=2)
    args = parser.parse_args()
    version = version_name(args.version)
    if platform.system() != 'Darwin' or platform.machine() != 'arm64' or args.jobs < 1:
        parser.error('Requires native Apple Silicon macOS and positive --jobs')
    if args.output.exists() or not args.nucleon.is_file():
        parser.error('Output must not exist and --nucleon must be a built CLI')
    experiment = Experiment(args.work.resolve(), args.jobs, 'tolkara')
    if args.action == 'build':
        experiment.fetch()
        if any(not tool for tool in experiment.doctor().values()):
            raise ValueError('Missing build tools')
        prepare_publisher_sources(experiment, version)
        experiment.build_fex()
        # A fresh install tree prevents leftover developer DLLs entering a release.
        previous = experiment.work / 'runtime'
        if previous.exists():
            retained = Path(tempfile.mkdtemp(prefix='retained-runtime-', dir=experiment.work))
            previous.rename(retained / 'runtime')
        experiment.build_wine()
        with tempfile.TemporaryDirectory(prefix='publisher-sources-', dir=experiment.work) as temporary:
            source_snapshot(experiment, Path(temporary))
            write_json(experiment.work / 'publisher-build.json',
                       build_identity(experiment, Path(temporary) / 'SOURCE-MANIFEST.json'))
    print(package(experiment, version, args.output.resolve(), args.nucleon.resolve()))


if __name__ == '__main__':
    main()
