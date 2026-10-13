#!/usr/bin/env python3
"""Rebuild only wow64cpu.dll for an otherwise unchanged Wine 11.18 runtime.

Prerequisites: Xcode command line tools, mingw-w64, bison >= 3, flex, make.
Example: brew install mingw-w64 bison
The workaround is LGPL-2.1-or-later, from dttdrv/wiage's cpu.c (CW HACK 20760).
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
PATCH = Path(__file__).resolve().parents[1] / 'assets/patches/wine-11.18-wow64cpu-rosetta.patch'

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('wine_root', type=Path)
    parser.add_argument('output', type=Path, help='New runtime directory; must not already exist')
    parser.add_argument('--source-archive', type=Path, help='Optional cached Wine 11.18 source archive')
    args = parser.parse_args()
    base = args.wine_root.resolve(strict=True)
    version = subprocess.check_output([str(base/'bin/wine'), '--version'], text=True).strip()
    if version != 'wine-11.18':
        parser.error(f'Expected plain wine-11.18, got {version!r}; do not mix module versions')
    if args.output.exists():
        parser.error('Output already exists; choose a new immutable runtime directory')
    environment = dict(os.environ)
    environment['PATH'] = '/opt/homebrew/opt/bison/bin:/opt/homebrew/bin:' + environment.get('PATH', '')
    with tempfile.TemporaryDirectory(prefix='nucleon-wow64-') as temporary:
        work = Path(temporary)
        archive = args.source_archive
        if archive is None:
            archive = work/'wine-11.18.tar.xz'
            urllib.request.urlretrieve(SOURCE_URL, archive)
        if hashlib.sha256(archive.read_bytes()).hexdigest() != SOURCE_SHA256:
            raise SystemExit('Wine source archive checksum mismatch')
        subprocess.run(['tar', '-xf', str(archive.resolve()), '-C', str(work)], check=True)
        source = work/'wine-11.18'
        subprocess.run(['patch', '--batch', '--fuzz=0', '-p1', '-i', str(PATCH)], cwd=source, check=True)
        build = work/'build'
        build.mkdir()
        subprocess.run([str(source/'configure'), '--enable-archs=i386,x86_64', '--without-x',
                        '--without-freetype', '--host=x86_64-apple-darwin',
                        'CC=clang -arch x86_64', 'CXX=clang++ -arch x86_64'],
                       cwd=build, env=environment, check=True)
        subprocess.run(['make', '-j8', 'dlls/wow64cpu/x86_64-windows/wow64cpu.dll'],
                       cwd=build, env=environment, check=True)
        module = build/'dlls/wow64cpu/x86_64-windows/wow64cpu.dll'
        args.output.parent.mkdir(parents=True, exist_ok=True)
        # Build beside the destination, then publish the complete copy with one rename.
        with tempfile.TemporaryDirectory(prefix='.wine-rosetta-', dir=args.output.parent) as staging:
            runtime = Path(staging)/'wine'
            shutil.copytree(base, runtime, symlinks=True)
            destination = runtime/'lib/wine/x86_64-windows/wow64cpu.dll'
            destination.unlink()
            shutil.copy2(module, destination)
            (runtime/'nucleon-runtime.json').write_text(json.dumps({
                'wine': version, 'base': str(base), 'source_url': SOURCE_URL,
                'source_sha256': SOURCE_SHA256,
                'patch_sha256': hashlib.sha256(PATCH.read_bytes()).hexdigest(),
                'wow64cpu_sha256': hashlib.sha256(module.read_bytes()).hexdigest(),
            }, indent=2)+'\n')
            os.rename(runtime, args.output)
    print(f'Register with: nucleon wine add wine-devel-11.18-rosetta "{args.output}" --use-now')

if __name__ == '__main__':
    main()
