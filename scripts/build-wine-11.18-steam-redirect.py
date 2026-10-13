#!/usr/bin/env python3
"""Build an experimental, separate Wine 11.18 runtime for native Steam DLL routing.

Requires Xcode command line tools, mingw-w64, bison >= 3, flex, make.
Changes only x86_64-windows/ntdll.dll. It never selects a global Wine runtime.
The loader patch is LGPL-2.1-or-later; see the Wine loader source header.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request

SOURCE_URL = 'https://dl.winehq.org/wine/source/11.x/wine-11.18.tar.xz'
SOURCE_SHA256 = 'c6282f6d4daecf11f3ab9f9a2b6c598f67be7e28c363ba2cae8a3c44b611e7a4'
PATCH = Path(__file__).resolve().parents[1] / 'assets/patches/wine-11.18-steamclient-redirect-experimental.patch'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('wine_root', type=Path, help='The pinned Wine 11.18 runtime to clone')
    parser.add_argument('output', type=Path, help='New runtime directory; must not exist')
    parser.add_argument('--source-archive', type=Path, help='Optional cached official Wine source')
    args = parser.parse_args()
    base = args.wine_root.resolve(strict=True)
    output = args.output.absolute()
    if output.exists():
        parser.error('Output exists; choose a new runtime directory')
    environment = dict(os.environ)
    for key in list(environment):
        if key.startswith(('DYLD_', 'WINE')):
            environment.pop(key)
    environment['PATH'] = '/opt/homebrew/opt/bison/bin:/opt/homebrew/bin:' + environment.get('PATH', '')
    version = subprocess.check_output([str(base / 'bin/wine'), '--version'],
                                      env=environment, text=True, timeout=10).strip()
    if version != 'wine-11.18':
        parser.error(f'Expected plain wine-11.18, got {version!r}; module versions must match')
    with tempfile.TemporaryDirectory(prefix='nucleon-steam-loader-') as temporary:
        work = Path(temporary)
        archive = args.source_archive
        if archive is None:
            archive = work / 'wine-11.18.tar.xz'
            urllib.request.urlretrieve(SOURCE_URL, archive)
        if digest(archive) != SOURCE_SHA256:
            raise SystemExit('Wine source archive checksum mismatch')
        subprocess.run(['tar', '-xf', str(archive.resolve()), '-C', str(work)], check=True)
        source = work / 'wine-11.18'
        subprocess.run(['patch', '--batch', '--fuzz=0', '-p1', '-i', str(PATCH)],
                       cwd=source, check=True)
        build = work / 'build'
        build.mkdir()
        subprocess.run([str(source / 'configure'), '--enable-archs=i386,x86_64',
                        '--without-x', '--without-freetype', '--host=x86_64-apple-darwin',
                        'CC=clang -arch x86_64', 'CXX=clang++ -arch x86_64'],
                       cwd=build, env=environment, check=True)
        subprocess.run(['make', '-j8', 'dlls/ntdll/x86_64-windows/ntdll.dll'],
                       cwd=build, env=environment, check=True)
        module = build / 'dlls/ntdll/x86_64-windows/ntdll.dll'
        relative = Path('lib/wine/x86_64-windows/ntdll.dll')
        output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='.wine-steam-loader-', dir=output.parent) as staging:
            runtime = Path(staging) / 'wine'
            shutil.copytree(base, runtime, symlinks=True)
            destination = runtime / relative
            destination.unlink()
            shutil.copy2(module, destination)
            (runtime / 'nucleon-steam-loader.json').write_text(json.dumps({
                'experimental': True, 'wine': version, 'base': str(base),
                'source_url': SOURCE_URL, 'source_sha256': SOURCE_SHA256,
                'patch_sha256': digest(PATCH),
                'base_ntdll_sha256': digest(base / relative),
                'ntdll_sha256': digest(destination),
                'activation': 'NUCLEON_STEAMCLIENT_REDIRECT=1',
            }, indent=2) + '\n')
            os.rename(runtime, output)
    print(f'Built private runtime: {output}')
    print('Use only in a game-specific profile with original Valve steamclient64.dll and lsteamclient.dll.')


if __name__ == '__main__':
    main()
