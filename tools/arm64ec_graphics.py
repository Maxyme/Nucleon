#!/usr/bin/env python3
"""Build and test native frontends without changing an installed runtime or game.

Only fetch uses the network. Probes use dedicated prefixes, AMD64 callers and
the existing matched ARM64 Wine/Vulkan/Metal boundary. No Steam/game is started.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import struct
import subprocess
import tempfile
import time
import tomllib

from arm64_fex import REPO, apply_wine_patches, atomic_copy, checkout, output, run, sha, source_diff, write_json
from arm64_fex_dx12 import DX12, windows_path
from nucleon_fex import configuration_lock

RECIPE = REPO / 'assets/wine/recipes/arm64ec-graphics.toml'


def image_identity(path):
    data = path.read_bytes()
    if len(data) < 64 or data[:2] != b'MZ': raise ValueError(f'Invalid frontend PE: {path}')
    pe = struct.unpack_from('<I', data, 60)[0]
    if len(data) < pe + 92 or data[pe:pe+4] != b'PE\0\0' or struct.unpack_from('<H', data, pe+24)[0] != 0x20b:
        raise ValueError(f'Invalid frontend PE64: {path}')
    stamp = struct.unpack_from('<I', data, pe+8)[0]
    size, checksum = struct.unpack_from('<I', data, pe+80)[0], struct.unpack_from('<I', data, pe+88)[0]
    return f'{stamp:08x}:{size:08x}:{checksum:08x}'


def cross_file(toolchain, architecture):
    if architecture not in ('arm64ec', 'x64'): raise ValueError('Unsupported frontend architecture')
    triple = 'arm64ec' if architecture == 'arm64ec' else 'x86_64'
    family = 'aarch64' if architecture == 'arm64ec' else 'x86_64'
    tools = dict(c='gcc', cpp='g++', ar='ar', strip='strip', windres='windres', widl='widl')
    lines = ['[binaries]']
    for name, suffix in tools.items():
        path = toolchain / 'bin' / f'{triple}-w64-mingw32-{suffix}'
        if not path.is_file(): raise ValueError(f'Missing compiler tool: {path}')
        lines.append(f'{name} = {str(path)!r}')
    return '\n'.join(lines + ["pkg-config = '/usr/bin/false'", '[properties]',
        'needs_exe_wrapper = true', '[host_machine]', "system = 'windows'",
        f"cpu_family = '{family}'", f"cpu = '{family}'", "endian = 'little'", ''])


class Frontends:
    def __init__(self, workspace, sdk, jobs, debug=False, native_llvm=None):
        self.base = DX12(workspace, sdk, jobs)
        self.base.debug = debug
        self.work = workspace / 'arm64ec'
        self.logs = self.work / 'logs'
        self.logs.mkdir(parents=True, exist_ok=True)
        self.pin = tomllib.loads(RECIPE.read_text())
        if self.pin['schema'] != 1: raise ValueError('Unsupported frontend recipe')
        self.env = self.base.env.copy()
        self.env['PATH'] = str(self.base.base.compiler / 'bin') + ':' + self.env['PATH']
        self.meson = workspace / 'dx12/venv/bin/meson'
        self.jobs = jobs
        # The paired DXMT source's CI uses LLVM 15.0.7 for its Metal translator.
        # DXMT recognizes Homebrew's opt path to link its static zstd/unwind
        # dependencies. Resolving the symlink to Cellar would skip that logic.
        self.native_llvm = (native_llvm or Path('/opt/homebrew/opt/llvm@15')).absolute()

    def source(self, name): return self.work / 'sources' / name

    def fetch(self):
        for name, pin in self.pin['sources'].items():
            source = self.source(name)
            if not (source / '.git').exists() or output(['git', '-C', source, 'rev-parse', 'HEAD']) != pin['revision']:
                checkout(source, pin)
            elif output(['git', '-C', source, 'status', '--porcelain', '--untracked-files=no']):
                self.verify_source(name)
            run(['git', '-C', source, 'submodule', 'update', '--init', '--recursive', '--depth', '1'])

    def verify_source(self, name):
        source = self.source(name)
        if output(['git', '-C', source, 'rev-parse', 'HEAD']) != self.pin['sources'][name]['revision']:
            raise ValueError(f'Source revision mismatch: {name}')
        if output(['git', '-C', source, 'status', '--porcelain', '--untracked-files=no']):
            patches = self.pin.get(name, {}).get('patches', [])
            marker = source / '.nucleon-overlay.json'
            overlay = json.loads(marker.read_text()) if marker.is_file() else {}
            if (not patches or overlay.get('diff_sha256') != source_diff(source)
                    or overlay.get('patches') != {p:sha(REPO / p) for p in patches}):
                raise ValueError(f'Modified source: {name}')
        modules = output(['git', '-C', source, 'submodule', 'status', '--recursive'])
        if any(line[0] in '-+U' for line in modules.splitlines()):
            raise ValueError(f'Missing or changed submodules: {name}; run fetch')
        return modules

    def build(self, name, architecture):
        if name == 'dxmt' and architecture != 'arm64ec':
            raise ValueError('DXMT needs an ARM64 Unix companion; use the existing paired AMD64 DXMT control, not an x64 Unix build')
        prerequisites = self.dxmt_prerequisites() if name == 'dxmt' else None
        modules = self.verify_source(name)
        patches = self.pin.get(name, {}).get('patches', [])
        if patches:
            apply_wine_patches(self.source(name), patches)
            self.verify_source(name)
        cross = self.work / f'{architecture}.ini'
        cross.write_text(cross_file(self.base.base.compiler, architecture))
        build = self.work / 'build' / architecture / name
        build.parent.mkdir(parents=True, exist_ok=True)
        log = self.logs / f'build-{name}-{architecture}.log'
        options = ['-Dbuildtype=release', '--wrap-mode=nodownload']
        if architecture == 'arm64ec':
            startup = self.darwin_startup()
            link = shlex.join(['-B' + str(startup), str(startup / 'emutls.o')])
            options += ['-Dc_args=' + shlex.join(self.teb_flags()),
                        '-Dcpp_args=' + shlex.join(self.teb_flags()),
                        '-Dc_link_args=' + link, '-Dcpp_link_args=' + link]
        if name == 'vkd3d': options += ['-Denable_tests=false', '-Denable_extras=false']
        if name == 'dxvk': options += ['-Denable_d3d8=false', '-Denable_d3d9=false', '-Denable_d3d10=false']
        if name == 'dxmt':
            # Native frontends must win normal DLL lookup. Wine-marked d3d11/
            # dxgi redirect builtin lookup to Wine's own copies before WINEDLLPATH.
            # winemetal retains its required builtin Unix-call bridge separately.
            options += ['-Denable_tests=false', '-Dwine_builtin_dll=false',
                        '-Dwine_build_path=' + str(self.base.work / 'wine-build'),
                        '-Dnative_llvm_path=' + str(self.native_llvm)]
        receipt = dict(status='BUILDING', architecture=architecture, source=self.pin['sources'][name],
                       submodules=modules, recipe_sha256=sha(RECIPE), cross_sha256=sha(cross),
                       toolchain=self.base.base.recipe['toolchain'], log=str(log))
        receipt['options'] = options
        if patches: receipt['overlay'] = json.loads((self.source(name) / '.nucleon-overlay.json').read_text())
        if prerequisites: receipt['prerequisites'] = prerequisites
        if architecture == 'arm64ec':
            receipt.update(crt_sha256=sha(startup / 'dllcrt2.o'),
                           teb_header_sha256=sha(REPO / 'tools/runtime-probes/darwin-teb.h'),
                           emutls_sha256=sha(startup / 'emutls.o'),
                           mingw_source=self.pin['sources']['mingw'])
        report = self.work / f'build-{name}-{architecture}.json'
        write_json(report, receipt)
        try:
            run([self.meson, 'setup', *(['--reconfigure'] if (build / 'build.ninja').exists() else []),
                 build, self.source(name), '--cross-file', cross, *options], env=self.env, log=log)
            run(['ninja', '-C', build, '-j', self.jobs], env=self.env, log=log)
            wanted = {'dxvk':['dxgi.dll','d3d11.dll'], 'vkd3d':['d3d12.dll','d3d12core.dll'],
                      'dxmt':['d3d11.dll','dxgi.dll','winemetal.dll','winemetal.so']}[name]
            stage = self.work / 'bundles' / architecture / name
            stage.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(prefix='.frontend-', dir=stage.parent) as temporary:
                temporary = Path(temporary)
                for filename in wanted:
                    candidates = list(build.rglob(filename))
                    if len(candidates) != 1: raise ValueError(f'Ambiguous/missing build output: {filename}')
                    if filename.endswith('.dll'):
                        headers = output([self.base.base.compiler / 'bin/llvm-readobj', '--file-headers', candidates[0]])
                        expected = 'COFF-ARM64EC' if architecture == 'arm64ec' else 'COFF-x86-64'
                        if expected not in headers: raise ValueError(f'Unexpected frontend ABI: {filename}')
                    atomic_copy(candidates[0], temporary / filename)
                receipt.update(status='BUILT_NOT_EXECUTION_VALIDATED',
                               files={n:sha(temporary / n) for n in wanted})
                write_json(temporary / 'build.json', receipt)
                if stage.exists():
                    # Only experiment-owned products; installed runtimes are never touched.
                    backup = stage.with_suffix('.previous')
                    if backup.exists(): shutil.rmtree(backup)
                    stage.rename(backup)
                temporary.rename(stage)
            write_json(report, receipt)
        except BaseException as error:
            receipt.update(status='BUILD_FAILED', error=str(error))
            write_json(report, receipt)
            raise

    def dxmt_prerequisites(self):
        checks, errors = {}, []
        for name, command, correction in [
            ('native_llvm', [self.native_llvm / 'bin/llvm-config', '--version'],
             'Install LLVM 15.0.7 (brew install llvm@15), or pass --native-llvm /path/to/llvm-15.0.7.'),
            ('metal', ['/usr/bin/xcrun', '-sdk', 'macosx', 'metal', '--version'],
             'Install the Xcode Metal compiler with: xcodebuild -downloadComponent MetalToolchain.'),
        ]:
            try:
                checks[name] = output(command, env=self.env)
                if name == 'native_llvm' and checks[name].strip() != self.pin['dxmt']['native_llvm_version']:
                    raise ValueError('Unexpected LLVM version: ' + checks[name])
            except (OSError, subprocess.SubprocessError, ValueError) as error:
                errors.append(f'DXMT prerequisite {name} is unavailable: {error}. {correction}')
        if errors: raise ValueError('\n'.join(errors))
        return checks

    def teb_flags(self):
        # LLVM's native PE TLS lowering also reads x18. Use the experiment's
        # Win32 TLS runtime, keeping per-thread storage and synchronization.
        return ['-femulated-tls', '-include', str(REPO / 'tools/runtime-probes/darwin-teb.h'),
                '-DNUCLEON_TEB_TSD_OFFSET=' + self.pin['darwin']['teb_tsd_offset']]

    def darwin_startup(self):
        self.verify_source('mingw')
        offset = self.pin['darwin']['teb_tsd_offset']
        wine_makefile = (self.base.work / 'wine-build/Makefile').read_text()
        fex_cache = (self.base.base.work / 'fex/arm64ec/CMakeCache.txt').read_text()
        if f'-D__WINE_TEB_TSD_OFFSET={offset} ' not in wine_makefile or f'FEX_TEB_TSD_OFFSET:STRING={offset}\n' not in fex_cache:
            raise ValueError('Darwin thread-pointer ABI differs from paired Wine/FEX')
        source = self.source('mingw') / 'mingw-w64-crt'
        startup = self.work / 'crt'
        startup.mkdir(exist_ok=True)
        run([self.base.base.compiler / 'bin/arm64ec-w64-mingw32-gcc', '-O2', '-c',
             source / 'crt/crtdll.c', '-o', startup / 'dllcrt2.o', '-D_SYSCRT', '-DCRTDLL',
             '-D_UCRT', '-I' + str(source / 'include'), *self.teb_flags()],
            env=self.env, log=self.logs / 'crt-build.log')
        run([self.base.base.compiler / 'bin/arm64ec-w64-mingw32-gcc', '-O2', '-c',
             REPO / 'tools/runtime-probes/darwin-emutls.c', '-o', startup / 'emutls.o',
             '-Wall', '-Wextra', '-Werror', *self.teb_flags()], env=self.env, log=self.logs / 'crt-build.log')
        return startup

    def probe(self, architecture, api):
        """Use identical AMD64 test callers for the native/translated comparison."""
        names = {'dx12':['dxvk','vkd3d'], 'dx11':['dxmt'], 'dx11-vulkan':['dxvk']}[api]
        bundles = [self.work / 'bundles' / architecture / name for name in names]
        inputs = {}
        for bundle in bundles:
            report = json.loads((bundle / 'build.json').read_text())
            if report.get('architecture') != architecture or report.get('status') != 'BUILT_NOT_EXECUTION_VALIDATED':
                raise ValueError(f'Wrong or incomplete frontend build: {bundle}')
            for name, expected in report['files'].items():
                if sha(bundle / name) != expected: raise ValueError(f'Modified frontend: {bundle / name}')
            inputs[bundle.name] = report
        label = f'frontend-{architecture}-{api}'
        loader, env = self.base.execution_environment(label)
        if self.base.debug: env['WINEDEBUG'] = '-all,err+all,+loaddll,+seh,+unwind'
        # This probe owns its prefix and wineserver; it never uses a game prefix.
        run_directory = self.work / 'runs' / label / f'{time.time_ns()}-{os.getpid()}'
        run_directory.mkdir(parents=True)
        log = run_directory / 'setup.log'
        log.write_bytes(b'')
        receipt = dict(status='TESTING', architecture=architecture, api=api, bundles=inputs,
                       wine=json.loads((self.base.work / 'runtime/build.json').read_text()),
                       driver_sha256=sha(self.base.work / 'driver/libvulkan_kosmickrisp.dylib'),
                       gameplay_validation='NOT_RUN', steam_validation='NOT_RUN', log=str(log), passed=[])
        runtime = self.base.work / 'runtime'
        for name, expected in receipt['wine']['host_files'].items():
            if sha(runtime / name) != expected: raise ValueError(f'Modified Wine boundary: {name}')
        receipt['boundary_files'] = {name:sha(runtime / name) for name in [
            'lib/wine/aarch64-windows/ntdll.dll',
            'lib/wine/aarch64-windows/winevulkan.dll',
            'lib/wine/aarch64-windows/libarm64ecfex.dll',
            'lib/wine/aarch64-unix/winevulkan.so']}
        report = run_directory / 'receipt.json'
        program = self.work / f'probe-{api}-x64.exe'
        compiler = self.base.base.compiler / 'bin' / ('x86_64-w64-mingw32-g++' if api == 'dx12' else 'x86_64-w64-mingw32-gcc')
        flags = ['-ld3d12','-ldxgi','-ld3dcompiler','-ldxguid','-luser32','-static'] if api == 'dx12' else ['-ld3d11','-ldxgi','-ld3dcompiler','-ldxguid','-municode','-static']
        run([compiler, '-O2', '-Wall', '-Wextra', '-Werror', REPO / ('tools/runtime-probes/d3d12.cpp' if api == 'dx12' else 'tools/runtime-probes/d3d11.c'), '-o', program, *flags], env=self.env, log=log)
        shader_compiler = Path(os.environ.get('NUCLEON_PROBE_D3DCOMPILER', str(Path.home() /
            'Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0/d3dcompiler_47.dll')))
        if not shader_compiler.is_file(): raise ValueError('Set NUCLEON_PROBE_D3DCOMPILER to a licensed x64 d3dcompiler_47.dll')
        atomic_copy(shader_compiler, program.parent / 'd3dcompiler_47.dll')
        receipt.update(probe_sha256=sha(program), shader_compiler_sha256=sha(shader_compiler))
        write_json(report, receipt)
        try:
            run([loader, 'wineboot', '-i'], env=env, log=log, timeout=120)
            system = Path(env['WINEPREFIX']) / 'drive_c/windows/system32'
            for bundle in bundles:
                for dll in bundle.glob('*.dll'):
                    atomic_copy(dll, system / dll.name)
                    env['NUCLEON_IMAGE_' + dll.name.upper().replace('.', '_')] = image_identity(dll)
            if api == 'dx11':
                env['WINEDLLOVERRIDES'] = 'd3d11,dxgi,winemetal=n,b;d3d12,d3d12core=b'
                native = bundles[0] / 'winemetal.so'
                atomic_copy(native, system / 'winemetal.so')
                env['WINEDLLPATH'] = str(system) + ':' + env['WINEDLLPATH']
                # Metal needs only its paired companion and Wine's Unix modules.
                # Vulkan/SDK search paths inflate Wine's optional FEX .so warning
                # past this runtime's debug buffer and can select unrelated libs.
                env['DYLD_LIBRARY_PATH'] = str(system) + ':' + str(runtime / 'lib/wine/aarch64-unix')
                env['DYLD_FALLBACK_LIBRARY_PATH'] = env['DYLD_LIBRARY_PATH']
                for name in ['VK_DRIVER_FILES', 'VK_ICD_FILENAMES', 'VK_LOADER_DEBUG']:
                    env.pop(name, None)
                env['NUCLEON_EXPECT_METAL'] = '1'
                receipt['driver_sha256'] = None
                receipt['graphics_path'] = 'metal'
            elif api == 'dx11-vulkan':
                env['WINEDLLOVERRIDES'] = 'vulkan-1,winevulkan=b;d3d11,dxgi=n,b'
            else:
                env['WINEDLLOVERRIDES'] += ';d3d11=n,b'
            env['NUCLEON_EXPECT_GRAPHICS_ABI'] = architecture
            env['WINEDLLOVERRIDES'] += ';d3dcompiler_47=n'
            receipt['launch_settings'] = {name:env[name] for name in [
                'WINEDEBUG', 'WINEDLLOVERRIDES', 'WINEDLLPATH', 'DYLD_LIBRARY_PATH']}
            receipt['graphics_image_identities'] = {k:v for k,v in env.items() if k.startswith('NUCLEON_IMAGE_')}
            # Cache format/content belongs to this frontend set, not the baseline.
            env['VKD3D_SHADER_CACHE_PATH'] = windows_path(self.work / 'caches' / label)
            (self.work / 'caches' / label).mkdir(parents=True, exist_ok=True)
            for phase, args in [('draw', []), ('control', ['--no-draw'])]:
                phase_log = log.with_name(log.stem + '-' + phase + '.log')
                phase_log.write_bytes(b'')
                run([loader, windows_path(program), *args], env=env, cwd=self.work,
                    log=phase_log, timeout=90, expected=0 if phase == 'draw' else 21)
                text = phase_log.read_text(errors='replace')
                marker = ('NUCLEON_D3D12_OK' if api == 'dx12' else 'NUCLEON_D3D11_TRIANGLE_OK') if phase == 'draw' else (
                    'NUCLEON_D3D12_FAIL triangle-pixels' if api == 'dx12' else 'NUCLEON_D3D11_FAIL triangle-stage=21')
                if marker not in text: raise ValueError(f'Missing probe evidence: {marker}')
                receipt['passed'].append(phase)
                write_json(report, receipt)
            if api == 'dx11':
                wrong = env.copy()
                wrong['NUCLEON_IMAGE_D3D11_DLL'] = '00000000:00000000:00000000'
                negative = run_directory / 'wrong-component.log'
                negative.write_bytes(b'')
                run([loader, windows_path(program)], env=wrong, cwd=self.work,
                    log=negative, timeout=90, expected=6)
                if 'NUCLEON_FRONTEND_FAIL wrong component d3d11.dll' not in negative.read_text(errors='replace'):
                    raise ValueError('Component identity negative control failed for an unexpected reason')
                receipt['component_negative_control'] = 'EXPECTED_COMPONENT_MISMATCH_EXIT_6'
            receipt['status'] = 'GPU_PROBES_PASSED_NOT_GAME_VALIDATED'
        except BaseException as error:
            receipt.update(status='FAILED', error=str(error))
            raise
        finally:
            try:
                self.stop_prefix(env, log)
            except BaseException as error:
                receipt.update(status='CLEANUP_FAILED', cleanup_error=str(error))
                raise
            finally:
                write_json(report, receipt)
                write_json(self.work / f'{label}.json', receipt)
                print(report, flush=True)

    def stop_prefix(self, env, log):
        server = self.base.work / 'runtime/bin/wineserver'
        run([server, '-k'], env=env, log=log, timeout=10)
        run([server, '-w'], env=env, log=log, timeout=10)

    def prepare_runtime(self, baseline=None):
        from arm64ec_runtime import assemble
        target = assemble(baseline or self.base.prepare_runtime(), self.work, self.pin)
        print(target, flush=True)
        return target

    def probe_tls(self):
        startup = self.darwin_startup()
        directory = self.work / 'tls-probe'
        directory.mkdir(exist_ok=True)
        compiler = self.base.base.compiler / 'bin/arm64ec-w64-mingw32-gcc'
        log = self.logs / 'tls-probe.log'
        log.write_bytes(b'')
        run([compiler, '-O2', '-shared', '-Wall', '-Wextra', '-Werror', '-DNUCLEON_TLS_TEST',
             *self.teb_flags(), '-B' + str(startup),
             REPO / 'tools/runtime-probes/frontend-tls.c',
             REPO / 'tools/runtime-probes/darwin-emutls.c', '-static',
             '-o', directory / 'frontend-tls.dll'], env=self.env, log=log)
        run([self.base.base.compiler / 'bin/x86_64-w64-mingw32-gcc', '-O2', '-Wall', '-Wextra', '-Werror',
             REPO / 'tools/runtime-probes/frontend-tls-caller.c', '-static',
             '-o', directory / 'caller.exe'], env=self.env, log=log)
        loader, env = self.base.execution_environment('frontend-tls')
        env['WINEDEBUG'] = '-all,err+all'
        receipt = dict(status='TESTING', dll_sha256=sha(directory / 'frontend-tls.dll'),
                       caller_sha256=sha(directory / 'caller.exe'), log=str(log))
        try:
            run([loader, 'wineboot', '-i'], env=env, log=log, timeout=120)
            env['WINEDLLOVERRIDES'] += ';frontend-tls=n'
            run([loader, windows_path(directory / 'caller.exe')], env=env, cwd=directory, log=log, timeout=60)
            if 'NUCLEON_FRONTEND_TLS_PASS' not in log.read_text(errors='replace'):
                raise ValueError('Missing multithreaded TLS evidence')
            receipt['status'] = 'TLS_PROBE_PASSED'
        except BaseException as error:
            receipt.update(status='FAILED', error=str(error))
            raise
        finally:
            try:
                self.stop_prefix(env, log)
            except BaseException as error:
                receipt.update(status='CLEANUP_FAILED', cleanup_error=str(error))
                raise
            finally:
                write_json(self.work / 'tls-result.json', receipt)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['fetch','build','probe','probe-tls','doctor','prepare-runtime'])
    parser.add_argument('--base-runtime', type=Path, help='Existing descriptor-backed AMD64-frontend runtime to copy without rebuilding')
    parser.add_argument('--component', choices=['dxvk','vkd3d','dxmt'], default='vkd3d')
    parser.add_argument('--architecture', choices=['arm64ec','x64'], default='arm64ec')
    parser.add_argument('--api', choices=['dx11','dx11-vulkan','dx12'], default='dx12')
    parser.add_argument('--work', type=Path, default=REPO / 'target/arm64-fex')
    parser.add_argument('--sdk', type=Path, default=Path.home() / 'VulkanSDK/1.4.363.0/macOS')
    parser.add_argument('--jobs', type=int, default=6)
    parser.add_argument('--debug', action='store_true')
    parser.add_argument('--native-llvm', type=Path, help='DXMT native LLVM 15.0.7 build prefix')
    args = parser.parse_args()
    experiment = Frontends(args.work.resolve(), args.sdk, args.jobs, args.debug, args.native_llvm)
    with configuration_lock(experiment.work / 'experiment'):
        if args.action == 'fetch': experiment.fetch()
        elif args.action == 'doctor': print(json.dumps(experiment.dxmt_prerequisites(), indent=2))
        elif args.action == 'build': experiment.build(args.component, args.architecture)
        elif args.action == 'probe-tls': experiment.probe_tls()
        elif args.action == 'prepare-runtime': experiment.prepare_runtime(args.base_runtime)
        else: experiment.probe(args.architecture, args.api)


if __name__ == '__main__': main()
