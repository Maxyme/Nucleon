#!/usr/bin/env python3
"""Build and exercise the isolated ARM64 Wine/FEX Direct3D 12 experiment."""
import argparse
import copy
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib

from arm64_fex import (Experiment, REPO, apply_wine_patches, atomic_copy, checkout, output,
                       run, sha, source_diff, write_json)

RECIPE = REPO / 'assets/wine/recipes/arm64-fex-dx12.toml'


def windows_path(path):
    return 'Z:' + str(path.resolve()).replace('/', '\\')


class DX12:
    def __init__(self, workspace, sdk, jobs):
        self.base = Experiment(workspace, jobs, 'tolkara')
        self.work = workspace / 'dx12'
        self.sources = self.work / 'sources'
        self.logs = self.work / 'logs'
        self.logs.mkdir(parents=True, exist_ok=True)
        self.sdk = sdk.resolve()
        self.jobs = jobs
        self.pin = tomllib.loads(RECIPE.read_text())
        if self.pin['schema'] != 1 or jobs < 1:
            raise ValueError('Unsupported DX12 recipe or invalid job count')
        self.env = self.base.env.copy()
        self.env['PATH'] = str(self.work / 'venv/bin') + ':/opt/homebrew/opt/llvm/bin:' + self.env['PATH']
        self.wine = self.sources / 'wine'
        self.probes = self.work / 'probes'
        self.probes.mkdir(exist_ok=True)

    def fetch(self):
        self.sources.mkdir(exist_ok=True)
        for name in ['steamac', 'mesa']:
            checkout(self.sources / name, self.pin[name])
        fetcher = copy.copy(self.base)
        fetcher.workspace = self.work / 'archives'
        fetcher.workspace.mkdir(exist_ok=True)
        for pin in self.pin['archives'].values():
            fetcher.fetch_archive(pin)
        if not (self.wine / '.git').exists():
            run(['git', 'clone', '--shared', '--no-checkout', self.base.wine, self.wine])
            run(['git', '-C', self.wine, 'checkout', '--detach',
                 self.base.recipe['alternatives']['tolkara']['wine']['revision']])

    def wine_experiment(self):
        e = Experiment(self.base.workspace, self.jobs, 'tolkara')
        e.work, e.wine, e.logs = self.work, self.wine, self.logs
        link = self.work / 'fex'
        if not link.exists():
            link.symlink_to(self.base.work / 'fex', target_is_directory=True)
        return e

    def build_wine(self):
        revision = self.base.recipe['alternatives']['tolkara']['wine']['revision']
        if output(['git','-C',self.wine,'rev-parse','HEAD']) != revision:
            raise ValueError('Unexpected Wine revision; preserve this checkout')
        settings = self.base.recipe['wine']['tolkara']
        helper = settings['shared_data_helper']
        if sha(self.base.wine / helper) != settings['shared_data_helper_sha256']:
            raise ValueError('Unexpected shared-data helper')
        shutil.copy2(self.base.wine / helper, self.wine / helper)
        run(['git', '-C', self.wine, 'add', '--intent-to-add', helper])
        patches = settings['patches'] + self.pin['wine']['patches']
        apply_wine_patches(self.wine, patches)
        env = self.env.copy()
        env.update(CFLAGS='-O2 -arch arm64', CXXFLAGS='-O2 -arch arm64',
                   CPPFLAGS=f'-I{self.sdk}/include', LDFLAGS=f'-arch arm64 -L{self.sdk}/lib',
                   DYLD_LIBRARY_PATH=str(self.sdk / 'lib'), PKG_CONFIG='/usr/bin/false',
                   PKG_CONFIG_LIBDIR='/nonexistent')
        build = self.work / 'wine-build'
        build.mkdir(exist_ok=True)
        configure = [x for x in self.base.recipe['wine']['configure'] if x != '--without-vulkan']
        if not (build / 'Makefile').exists():
            run([self.wine / 'configure', '--prefix=/', '--bindir=/bin', '--libdir=/lib',
                 '--enable-archs=arm64ec,aarch64,i386', f'--with-mingw={self.base.compiler}/bin/clang',
                 *configure, '--with-vulkan', 'enable_amd_ags_x64=no'], cwd=build,
                env=env, log=self.logs / 'wine-configure.log')
        config = (build / 'include/config.h').read_text()
        if not re.search(r'^#define SONAME_LIBVULKAN "libvulkan', config, re.M):
            raise ValueError('Wine must use the Khronos Vulkan loader, not a direct ICD')
        run(['make', f'-j{self.jobs}'], cwd=build, env=env, log=self.logs / 'wine-build.log')
        run(['make', 'install', f'DESTDIR={self.work}/runtime'], cwd=build,
            env=env, log=self.logs / 'wine-install.log')
        e = self.wine_experiment()
        e.record_wine_build(patches)
        atomic_copy(RECIPE, self.work / 'runtime/share/nucleon-build' / RECIPE.name)
        receipt = self.work / 'runtime/build.json'
        value = json.loads(receipt.read_text())
        boundaries = ['lib/wine/aarch64-windows/ntdll.dll', 'lib/wine/aarch64-windows/winevulkan.dll',
                      'lib/wine/aarch64-windows/vulkan-1.dll', 'lib/wine/aarch64-windows/user32.dll',
                      'lib/wine/aarch64-unix/winevulkan.so',
                      'lib/wine/aarch64-unix/win32u.so']
        value['graphics_boundary_files'] = {name:sha(self.work / 'runtime' / name) for name in boundaries}
        value.update(dx12_recipe_sha256=sha(RECIPE), vulkan_sdk=str(self.sdk),
                     vulkan_loader_sha256=sha(self.sdk / 'lib/libvulkan.1.dylib'))
        write_json(receipt, value)

    def prepare_mesa(self):
        mesa = self.sources / 'mesa'
        for name in ['steamac','mesa']:
            if output(['git','-C',self.sources / name,'rev-parse','HEAD']) != self.pin[name]['revision']:
                raise ValueError('Unexpected source revision: ' + name)
        marker = mesa / '.nucleon-overlay.json'
        patches = sorted((self.sources / 'steamac/host/kosmickrisp/patches').glob('*.patch'))
        if not patches:
            raise ValueError('Steamac patches have not been fetched')
        hashes = {p.name: sha(p) for p in patches}
        if marker.exists():
            previous = json.loads(marker.read_text())
            if previous['diff_sha256'] != source_diff(mesa) or previous['patches'] != hashes:
                raise ValueError('Preserve modified Mesa checkout; source/patches changed')
            return
        if output(['git', '-C', mesa, 'status', '--porcelain']):
            raise ValueError('Preserve modified Mesa source checkout')
        for patch in patches:
            run(['git', '-C', mesa, 'apply', '--whitespace=nowarn', patch], log=self.logs / 'mesa-patches.log')
        # Include new files in the owned overlay receipt, without committing them.
        untracked = output(['git', '-C', mesa, 'ls-files', '--others', '--exclude-standard']).splitlines()
        if untracked:
            run(['git', '-C', mesa, 'add', '--intent-to-add', '--', *untracked])
        write_json(marker, dict(diff_sha256=source_diff(mesa), patches=hashes))

    def build_driver(self):
        self.prepare_mesa()
        venv = self.work / 'venv'
        if not (venv / 'bin/python3').exists():
            run([sys.executable, '-m', 'venv', venv])
        versions = subprocess.run([venv / 'bin/python3','-c',
            'import importlib.metadata,json;print(json.dumps({d.metadata["Name"].lower():d.version for d in importlib.metadata.distributions()}))'],
            text=True,capture_output=True,check=True)
        installed = json.loads(versions.stdout)
        if any(installed.get(k.lower()) != v for k,v in self.pin['python'].items()):
            run([venv / 'bin/python3', '-m', 'pip', 'install',
                 *[f'{k}=={v}' for k, v in self.pin['python'].items()]],
                log=self.logs / 'python-dependencies.log')
        src, clc = self.sources / 'mesa', self.work / 'clc'
        common = ['--buildtype=release', '-Db_ndebug=true', '-Dgallium-drivers=',
                  '-Dopengl=false', '-Dglx=disabled', '-Degl=disabled', '-Dgbm=disabled', '-Dzstd=disabled']
        build = self.work / 'mesa-clc-build'
        if not (build / 'build.ninja').exists():
            run(['meson', 'setup', build, src, f'--prefix={clc}', *common,
                 '-Dplatforms=', '-Dvulkan-drivers=', '-Dllvm=enabled', '-Dshared-llvm=enabled',
                 '-Dmesa-clc=enabled', '-Dinstall-mesa-clc=true',
                 '-Dprecomp-compiler=enabled', '-Dinstall-precomp-compiler=true'],
                env=self.env, log=self.logs / 'clc-configure.log')
        run(['ninja', '-C', build, f'-j{self.jobs}', 'install'], env=self.env, log=self.logs / 'clc-build.log')
        env = dict(self.env, PATH=str(clc / 'bin') + ':' + self.env['PATH'], MACOSX_DEPLOYMENT_TARGET='26.0')
        build = self.work / 'mesa-build'
        if not (build / 'build.ninja').exists():
            run(['meson', 'setup', build, src, *common, '-Dplatforms=macos',
                 '-Dvulkan-drivers=kosmickrisp', '-Dexpat=disabled', '-Dllvm=disabled',
                 '-Dspirv-tools=disabled', '-Dmesa-clc=system', '-Dprecomp-compiler=enabled', '--prefer-static'],
                env=env, log=self.logs / 'driver-configure.log')
        run(['ninja', '-C', build, f'-j{self.jobs}'], env=env, log=self.logs / 'driver-build.log')
        stage = self.work / 'driver'
        stage.mkdir(exist_ok=True)
        native = stage / 'libvulkan_kosmickrisp.dylib'
        atomic_copy(build / 'src/kosmickrisp/vulkan/libvulkan_kosmickrisp.dylib', native)
        run(['install_name_tool', '-id', '@rpath/libvulkan_kosmickrisp.dylib', native])
        run(['codesign', '--force', '--sign', '-', '--timestamp=none', native])
        run(['codesign', '--verify', '--strict', native])
        imports = output(['otool','-L',native]).splitlines()[2:]
        if any(not line.strip().startswith(('/usr/lib/','/System/')) for line in imports):
            raise ValueError('Driver has non-system runtime dependencies')
        if output(['lipo', '-archs', native]) != 'arm64':
            raise ValueError('Driver must be native ARM64')
        write_json(stage / 'kosmickrisp_icd.json', dict(file_format_version='1.0.1',
            ICD=dict(library_path='./libvulkan_kosmickrisp.dylib', api_version='1.4.0')))
        overlay = json.loads((src / '.nucleon-overlay.json').read_text())
        write_json(stage / 'build.json', dict(status='BUILT_NOT_EXECUTION_VALIDATED',
            recipe_sha256=sha(RECIPE), sources={k:self.pin[k] for k in ['steamac','mesa']},
            overlay=overlay, driver_sha256=sha(native), llvm=output(['llvm-config','--version'],env=self.env)))
        atomic_copy(src / 'docs/license.rst', stage / 'MESA-LICENSE.rst')
        license_data = subprocess.check_output(['git','-C',str(self.sources / 'steamac'),
            'show',self.pin['steamac']['revision'] + ':LICENSE'])
        (stage / 'STEAMAC-LICENSE').write_bytes(license_data)
        for patch in (self.sources / 'steamac/host/kosmickrisp/patches').glob('*.patch'):
            atomic_copy(patch,stage / 'source-patches' / patch.name)

    def graphics(self):
        stage = self.work / 'graphics'
        stage.mkdir(exist_ok=True)
        files = {}
        for name, members in [('vkd3d', ['x64/d3d12.dll', 'x64/d3d12core.dll']),
                              ('dxvk', ['x64/dxgi.dll'])]:
            pin = self.pin['archives'][name]
            archive = self.work / 'archives' / pin['archive']
            if sha(archive) != pin['sha256']:
                raise ValueError('Graphics archive checksum mismatch: ' + name)
            inventory = output(['/usr/bin/tar', '-tf', archive]).splitlines()
            for member in members:
                candidates = [x for x in inventory if x.endswith('/' + member)]
                if len(candidates) != 1:
                    raise ValueError('Ambiguous archive member: ' + member)
                data = subprocess.check_output(['/usr/bin/tar', '-xOf', str(archive), candidates[0]])
                target = stage / Path(member).name
                target.write_bytes(data)
                files[target.name] = sha(target)
            for member in inventory:
                if Path(member).name.lower() in ['license', 'copying', 'copying.lib']:
                    data = subprocess.check_output(['/usr/bin/tar', '-xOf', str(archive), member])
                    (stage / (name + '-' + Path(member).name)).write_bytes(data)
        write_json(stage / 'build.json', dict(archives=self.pin['archives'], files=files))
        return stage

    def execution_environment(self, prefix):
        e = self.wine_experiment()
        loader, env = e.runtime_environment(prefix)
        env['DYLD_LIBRARY_PATH'] = str(self.work / 'driver') + ':' + env['DYLD_LIBRARY_PATH'] + ':' + str(self.sdk / 'lib')
        env['VK_DRIVER_FILES'] = str(self.work / 'driver/kosmickrisp_icd.json')
        env['VK_ICD_FILENAMES'] = env['VK_DRIVER_FILES']
        env['WINEDLLOVERRIDES'] += ';vulkan-1,winevulkan=b;d3d12,d3d12core,dxgi=n,b'
        cache = self.work / 'caches' / prefix
        cache.mkdir(parents=True,exist_ok=True)
        env['VKD3D_SHADER_CACHE_PATH'] = str(cache)
        env['MESA_SHADER_CACHE_DIR'] = str(cache / 'mesa')
        env.update(VKD3D_DEBUG='info', DXVK_LOG_LEVEL='info',
                   DXVK_LOG_PATH=str(self.logs), WINEDEBUG='+loaddll', VK_LOADER_DEBUG='driver')
        for name in ['VKD3D_CONFIG','VKD3D_FEATURE_LEVEL','VKD3D_SHADER_MODEL',
                     'MVK_CONFIG_ADVERTISE_API_VERSION', 'MESA_VK_VERSION_OVERRIDE']:
            env.pop(name, None)
        return loader, env

    def stage_prefix(self, prefix):
        loader, env = self.execution_environment(prefix)
        stage = self.graphics()
        run([loader, 'wineboot', '-i'], env=env, log=self.logs / (prefix + '-prefix.log'), timeout=120)
        system = Path(env['WINEPREFIX']) / 'drive_c/windows/system32'
        for name in ['d3d12.dll','d3d12core.dll','dxgi.dll']:
            atomic_copy(stage / name, system / name)
        return loader, env

    def probe_driver(self):
        native = self.work / 'driver/libvulkan_kosmickrisp.dylib'
        run(['codesign','--verify','--strict',native])
        env = self.env.copy()
        env.update(VK_DRIVER_FILES=str(self.work / 'driver/kosmickrisp_icd.json'),
                   REPRO_DRIVER='kosmickrisp', MTL_DEBUG_LAYER='1', MTL_DEBUG_LAYER_ERROR_MODE='assert')
        env.pop('VK_ICD_FILENAMES', None)
        here = self.sources / 'steamac/host/moltenvk'
        def compile_native(source, target):
            run(['/usr/bin/clang','-arch','arm64','-std=c11','-Wall','-Werror','-O1',
                 '-I' + str(self.sdk / 'include'), source, '-L' + str(self.sdk / 'lib'),
                 '-lvulkan','-Wl,-rpath,' + str(self.sdk / 'lib'),'-o',target],
                env=self.env,log=self.logs / 'driver-tests-build.log')
        probe = self.probes / 'driver-caps'
        compile_native(here / 'probe/probe.c',probe)
        log = self.logs / 'driver-caps.log'
        log.write_bytes(b'')
        run([probe],env=env,log=log,timeout=60)
        shaders = self.probes / 'shaders'
        shaders.mkdir(exist_ok=True)
        build_log = self.logs / 'driver-shaders.log'
        for source in sorted((here / 'repro/shaders').iterdir()):
            if source.suffix in ['.vert','.geom','.frag','.comp']:
                args = ['--target-env','vulkan1.3'] if source.suffix == '.comp' else []
                run(['glslangValidator','-V','--quiet',*args,source,'-o',shaders / (source.name + '.spv')],
                    env=self.env,log=build_log)
            elif source.suffix == '.spvasm':
                run(['spirv-as','--target-env','vulkan1.0',source,'-o',shaders / (source.stem + '.spv')],
                    env=self.env,log=build_log)
        receipt = dict(driver_sha256=sha(native), status='TESTING', passed=['capabilities'], failures={})
        report = self.probes / 'driver-result.json'
        try:
            for name in ['geometry','robust_access','device_address','texel_buffer','queries','sparse']:
                program = self.probes / name
                compile_native(here / f'repro/{name}.c',program)
                log = self.logs / (name + '.log')
                log.write_bytes(b'')
                try:
                    run([program,shaders],env=env,log=log,timeout=180)
                    receipt['passed'].append(name)
                except (RuntimeError,subprocess.SubprocessError) as error:
                    receipt['failures'][name] = str(error)
                write_json(report,receipt)
            if receipt['failures']:
                raise ValueError('Native driver checks failed: ' + ', '.join(receipt['failures']))
            receipt['status'] = 'NATIVE_GPU_CHECKS_PASSED'
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            receipt.update(status='FAILED', error=str(error))
            raise
        finally:
            write_json(report,receipt)

    def stage_steam(self, loader, env, bridge, log):
        prefix = Path(env['WINEPREFIX'])
        steam = prefix / 'drive_c/Program Files (x86)/Steam'
        steam.mkdir(parents=True,exist_ok=True)
        atomic_copy(bridge / 'x86_64-windows/lsteamclient.dll',steam / 'lsteamclient.dll')
        atomic_copy(bridge / 'x86_64-windows/lsteamclient.dll',prefix / 'drive_c/windows/system32/lsteamclient.dll')
        atomic_copy(bridge / 'steamclient64.dll.valve',steam / 'steamclient64.dll')
        atomic_copy(bridge / 'steam.exe',steam / 'steam.exe')
        native = steam / 'lsteamclient.so'
        atomic_copy(bridge / 'aarch64-unix/lsteamclient.so',native)
        run(['install_name_tool','-change','@rpath/ntdll.so','@loader_path/ntdll.so',native])
        run(['codesign','--force','--sign','-','--timestamp=none',native])
        run(['codesign','--verify','--strict',native])
        ntdll = steam / 'ntdll.so'
        ntdll.unlink(missing_ok=True)
        ntdll.symlink_to(self.work / 'runtime/lib/wine/aarch64-unix/ntdll.so')
        env['WINEDLLPATH'] = str(steam) + ':' + env['WINEDLLPATH']
        env['STEAM_COMPAT_CLIENT_INSTALL_PATH'] = str(Path.home() /
            'Library/Application Support/Steam/Steam.AppBundle/Steam/Contents/MacOS')
        env['WINEDLLOVERRIDES'] += ';steamclient64=n;lsteamclient=n,b;nvapi,nvapi64,nvngx=d'
        env.update(SteamAppId='601150',SteamGameId='601150',NUCLEON_STEAMCLIENT_REDIRECT='1')
        # Registry names give the game's Steam SDK the real Valve image path.
        registry = [('Software\\Valve\\Steam','SteamPath','C:\\Program Files (x86)\\Steam'),
                    ('Software\\Valve\\Steam\\ActiveProcess','SteamClientDll64','C:\\Program Files (x86)\\Steam\\steamclient64.dll'),
                    ('Software\\Valve\\Steam\\ActiveProcess','Universe','Public')]
        for key,name,value in registry:
            run([loader,'reg','add','HKCU\\'+key,'/v',name,'/t','REG_SZ','/d',value,'/f'],env=env,log=log,timeout=30)
        return native, steam

    def probe_steam(self):
        bridge = Path(os.environ.get('NUCLEON_STEAM_BRIDGE', str(Path.home() / 'Library/Application Support/nucleon/bridge')))
        program = self.probes / 'steam-route-x64.exe'
        run([self.base.compiler / 'bin/x86_64-w64-mingw32-gcc','-O2','-Wall','-Wextra','-Werror',
             REPO / 'tools/runtime-probes/steam-route.c','-o',program],env=self.env)
        loader, env = self.stage_prefix('steam-route-smoke')
        log = self.logs / 'steam-route.log'
        log.write_bytes(b'')
        receipt = dict(status='TESTING')
        try:
            native, steam = self.stage_steam(loader,env,bridge,log)
            original = sha(steam / 'steamclient64.dll')
            api = Path(os.environ.get('NUCLEON_STEAM_API', str(Path.home() /
                'Library/Application Support/Steam/steamapps/common/Devil May Cry 5/steam_api64.dll')))
            if not api.is_file():
                raise ValueError('Set NUCLEON_STEAM_API to the installed game Steam API DLL')
            env['NUCLEON_STEAM_API'] = windows_path(api)
            run([loader,'reg','add',r'HKCU\Software\Valve\Steam\ActiveProcess','/v','pid',
                 '/t','REG_DWORD','/d','0','/f'],env=env,log=log,timeout=30)
            presence_negative = self.logs / 'steam-presence-negative.log'
            presence_negative.write_bytes(b'')
            run([loader,program],env=env,log=presence_negative,timeout=60,expected=15)
            if 'NUCLEON_STEAM_NOT_RUNNING' not in presence_negative.read_text(errors='replace'):
                raise ValueError('Steam presence negative control failed for an unexpected reason')
            positive = self.logs / 'steam-api-probe.log'
            positive.unlink(missing_ok=True)
            run([loader,steam / 'steam.exe',windows_path(program)],
                env=dict(env,NUCLEON_STEAM_RESULT=windows_path(positive)),log=log,timeout=60)
            if not positive.exists() or 'NUCLEON_STEAM_ROUTE_OK' not in positive.read_text(errors='replace'):
                raise ValueError('No successful Steam route marker')
            negative = self.logs / 'steam-route-negative.log'
            negative.write_bytes(b'')
            run([loader,program],env=dict(env,NUCLEON_STEAMCLIENT_REDIRECT='0'),log=negative,timeout=30,expected=10)
            if 'NUCLEON_STEAM_ROUTE_LOAD_FAILED' not in negative.read_text(errors='replace'):
                raise ValueError('Steam route negative control failed for an unexpected reason')
            if sha(steam / 'steamclient64.dll') != original:
                raise ValueError('Valve image changed on disk')
            receipt.update(status='NATIVE_STEAM_ROUTE_PASSED',valve_sha256=original,
                           unix_sha256=sha(native),steam_helper_sha256=sha(steam / 'steam.exe'),
                           steam_api_sha256=sha(api),presence_negative_control='EXPECTED_NOT_RUNNING_WITHOUT_HELPER',
                           negative_control='EXPECTED_LOAD_FAILURE_WITHOUT_REDIRECT')
        except (OSError,ValueError,RuntimeError,subprocess.SubprocessError) as error:
            receipt.update(status='FAILED',error=str(error))
            raise
        finally:
            self.wine_experiment().stop_server(env)
            write_json(self.probes / 'steam-route-result.json',receipt)

    def probe_vulkan(self):
        program = self.probes / 'vulkan-x64.exe'
        run([self.base.compiler / 'bin/x86_64-w64-mingw32-gcc','-O2','-Wall','-Wextra','-Werror',
             '-I' + str(self.sdk / 'include'), REPO / 'tools/runtime-probes/vulkan.c','-o',program],env=self.env)
        loader, env = self.execution_environment('vulkan-smoke')
        log = self.logs / 'vulkan.log'
        log.write_bytes(b'')
        receipt = dict(status='TESTING',probe_sha256=sha(program),steam_launched=False)
        try:
            run([loader,program],env=env,log=log,timeout=120)
            if 'NUCLEON_VULKAN_OK device-and-win32-surface' not in log.read_text(errors='replace'):
                raise ValueError('No successful Windows Vulkan marker')
            receipt['status'] = 'X64_VULKAN_DEVICE_AND_SURFACE_PASSED'
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            receipt.update(status='FAILED',error=str(error))
            raise
        finally:
            self.wine_experiment().stop_server(env)
            write_json(self.probes / 'vulkan-result.json',receipt)

    def probe_d3d12(self):
        source = REPO / 'tools/runtime-probes/d3d12.cpp'
        program = self.probes / 'd3d12-x64.exe'
        run([self.base.compiler / 'bin/x86_64-w64-mingw32-g++','-O2','-Wall','-Wextra','-Werror',
             '-static-libgcc','-static-libstdc++',source,'-o',program,
             '-ld3d12','-ldxgi','-ld3dcompiler','-ldxguid'],env=self.env)
        loader, env = self.stage_prefix('d3d12-smoke')
        compiler = Path(os.environ.get('NUCLEON_PROBE_D3DCOMPILER', str(Path.home() /
            'Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0/d3dcompiler_47.dll')))
        if not compiler.is_file():
            raise ValueError('Set NUCLEON_PROBE_D3DCOMPILER to a licensed x64 d3dcompiler_47.dll for SM5 probes')
        atomic_copy(compiler, self.probes / 'd3dcompiler_47.dll')
        env['WINEDLLOVERRIDES'] += ';d3dcompiler_47=n'

        report = self.probes / 'd3d12-result.json'
        receipt = dict(status='TESTING', probe_sha256=sha(program), shader_compiler_sha256=sha(compiler), steam_launched=False,
                       driver_sha256=sha(self.work / 'driver/libvulkan_kosmickrisp.dylib'),
                       graphics=json.loads((self.work / 'graphics/build.json').read_text()))
        try:
            log = self.logs / 'd3d12.log'
            log.write_bytes(b'')
            run([loader,program],env=env,cwd=self.probes,log=log,timeout=180)
            if 'NUCLEON_D3D12_OK' not in log.read_text(errors='replace'):
                raise ValueError('No successful D3D12 execution marker')
            negative = self.logs / 'd3d12-negative.log'
            negative.write_bytes(b'')
            run([loader,program,'--no-draw'],env=env,cwd=self.probes,log=negative,timeout=90,expected=21)
            if 'NUCLEON_D3D12_FAIL triangle-pixels' not in negative.read_text(errors='replace'):
                raise ValueError('D3D12 negative control failed for an unexpected reason')
            receipt.update(status='D3D12_GPU_AND_PRESENTATION_PASSED',
                           negative_control='EXPECTED_PIXEL_MISMATCH_WITHOUT_DRAW')
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            receipt.update(status='FAILED',error=str(error))
            raise
        finally:
            self.wine_experiment().stop_server(env)
            write_json(report,receipt)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=['fetch','build-wine','build-driver','probe-driver','probe-vulkan','probe-d3d12','probe-steam'])
    p.add_argument('--work', type=Path, default=REPO / 'target/arm64-fex')
    p.add_argument('--sdk', type=Path, default=Path.home() / 'VulkanSDK/1.4.363.0/macOS')
    p.add_argument('--jobs', type=int, default=6)
    a = p.parse_args()
    e = DX12(a.work.resolve(), a.sdk, a.jobs)
    getattr(e,a.action.replace('-','_'))()


if __name__ == '__main__':
    main()
