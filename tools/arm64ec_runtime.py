"""Assemble an unselected ARM64EC Vulkan generation without executing Wine/Steam."""
import hashlib
import json
from pathlib import Path
import shutil
import shlex
import tempfile
import tomllib

from arm64_fex import REPO, atomic_copy, run, sha, write_json


COMPONENTS = {'dxvk': ('d3d11.dll', 'dxgi.dll'),
              'vkd3d': ('d3d12.dll', 'd3d12core.dll')}
SHARED = Path('share/nucleon/arm64ec')


def paired_inputs(work, pin):
    """Check retained build products and the Darwin CRT/TLS objects before copying."""
    recipe = REPO / 'assets/wine/recipes/arm64ec-graphics.toml'
    builds, copies = {}, {}
    for component, names in COMPONENTS.items():
        bundle = work / 'bundles/arm64ec' / component
        build = json.loads((bundle / 'build.json').read_text())
        if (build.get('architecture') != 'arm64ec'
                or build.get('status') != 'BUILT_NOT_EXECUTION_VALIDATED'
                or build.get('source') != pin['sources'][component]
                or build.get('mingw_source') != pin['sources']['mingw']
                or set(build.get('files', {})) != set(names)
                or not all(any(option.startswith(prefix) and '-femulated-tls' in shlex.split(option[len(prefix):])
                               and '-DNUCLEON_TEB_TSD_OFFSET=' + pin['darwin']['teb_tsd_offset'] in shlex.split(option[len(prefix):])
                               for option in build.get('options', []))
                           for prefix in ['-Dc_args=', '-Dcpp_args='])):
            raise ValueError(f'Unpaired ARM64EC {component} build provenance')
        for name in names:
            source = bundle / name
            if sha(source) != build['files'][name]:
                raise ValueError(f'Modified ARM64EC frontend: {source}')
            copies[SHARED / component / name] = source
        copies[SHARED / component / 'build.json'] = bundle / 'build.json'
        for name, key, source in [
            ('dllcrt2.o', 'crt_sha256', work / 'crt/dllcrt2.o'),
            ('emutls.o', 'emutls_sha256', work / 'crt/emutls.o'),
            ('darwin-teb.h', 'teb_header_sha256', REPO / 'tools/runtime-probes/darwin-teb.h'),
        ]:
            if sha(source) != build.get(key):
                raise ValueError(f'ARM64EC {component} CRT/TLS mismatch: {source}')
            copies[SHARED / 'crt' / name] = source
        builds[component] = build
        # Keep the originating recipe digest: unrelated later DXMT recipe additions
        # do not invalidate pinned Vulkan sources, flags and CRT/TLS object identities.
    if builds['dxvk']['toolchain'] != builds['vkd3d']['toolchain']:
        raise ValueError('ARM64EC frontends use different compiler toolchains')
    copies[SHARED / recipe.name] = recipe
    copies[SHARED / 'crt/darwin-emutls.c'] = REPO / 'tools/runtime-probes/darwin-emutls.c'
    return builds, copies


def experimental_descriptor(original):
    """Keep the existing execution/Steam contract; scope graphics and guests explicitly."""
    descriptor = tomllib.loads(original)
    if descriptor.get('mode') != 'arm64-fex' or descriptor.get('graphics', {}).get('frontend_abi', 'x64') != 'x64':
        raise ValueError('ARM64EC assembly requires a baseline ARM64/FEX runtime with x64 frontends')
    descriptor['guests'] = ['x64']
    descriptor['windows_dirs'] = [p for p in descriptor['windows_dirs']
                                  if Path(p).name == 'aarch64-windows']
    if len(descriptor['windows_dirs']) != 1:
        raise ValueError('Expected one paired ARM64EC Windows directory')
    descriptor['backends'] = ['auto', 'dxvk', 'kosmickrisp']
    emulators = [e for e in descriptor['emulators'] if e['guest'] == 'x64']
    if len(emulators) != 1 or 'unix_library' not in emulators[0]:
        raise ValueError('Expected a complete x64 FEX emulator pair')
    prefix = dict(descriptor['prefix'])
    prefix['family'] += '-arm64ec'
    if len(prefix['family']) > 64:
        raise ValueError('Experimental prefix family exceeds 64 characters')
    graphics = dict(descriptor['graphics'])
    if not graphics.get('vulkan_icd') or not graphics.get('vulkan_loader'):
        raise ValueError('ARM64EC Vulkan assembly requires the paired driver and loader')
    graphics.pop('dxmt', None)
    graphics.update(frontend_abi='arm64ec', frontend_manifest=str(SHARED / 'frontend.json'),
                    dxvk=str(SHARED / 'dxvk'), vkd3d=str(SHARED / 'vkd3d'))
    lines = []
    for key in ['schema', 'mode', 'guests', 'loader', 'server', 'unix_dir', 'windows_dirs',
                'backends', 'steam_helper', 'steam_protocol']:
        if key in descriptor:
            lines.append(f'{key} = {json.dumps(descriptor[key])}')
    for heading, values in [('prefix', prefix), ('[emulators]', emulators[0]),
                            ('environment', dict(descriptor.get('environment', {}),
                                                 FEX_TSOENABLED='1', NUCLEON_EMULATE_KUSD='1')),
                            ('graphics', graphics)]:
        lines.append(f'[{heading}]')
        lines.extend(f'{key} = {json.dumps(value)}' for key, value in values.items())
    return '\n'.join(lines) + '\n'


def assemble(baseline, work, pin):
    baseline = baseline.resolve()
    original = (baseline / 'nucleon-runtime.toml').read_text()
    descriptor_text = experimental_descriptor(original)
    descriptor = tomllib.loads(descriptor_text)
    builds, copies = paired_inputs(work, pin)
    # Include the current native FEX companion, which older GPU receipts omitted.
    windows = Path(descriptor['windows_dirs'][0])
    unix = Path(descriptor['unix_dir'])
    graphics = descriptor['graphics']
    icd = Path(graphics['vulkan_icd'])
    declared_library = Path(json.loads((baseline / icd).read_text())['ICD']['library_path'])
    if declared_library.is_absolute():
        raise ValueError('Bundled Vulkan driver must use a relative library path')
    library = (baseline / icd.parent / declared_library).resolve().relative_to(baseline)
    for relative in [Path(descriptor['loader']), Path(descriptor['server']),
                     unix / 'ntdll.so', unix / 'winevulkan.so', unix / 'libarm64ecfex.so',
                     windows / 'ntdll.dll', windows / 'winevulkan.dll', windows / 'libarm64ecfex.dll',
                     icd, library, Path(graphics['vulkan_loader'])]:
        source = baseline / relative
        if relative.is_absolute() or not source.resolve().is_relative_to(baseline):
            raise ValueError(f'Runtime component escapes baseline: {relative}')
        copies[relative] = source
    contract = dict(schema=1, frontend_abi='arm64ec', builds=builds,
                    files={str(relative): sha(source) for relative, source in copies.items()})
    inputs = dict(baseline=str(baseline), descriptor=descriptor_text, pairing=contract,
                  validation='NOT_EXECUTION_VALIDATED', steam_validation='NOT_RUN', gameplay_validation='NOT_RUN')
    generation = hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest()[:20]
    parent = work.parent / 'arm64ec-integrated'
    parent.mkdir(parents=True, exist_ok=True)
    target = parent / generation
    if target.exists():
        if ((target / 'nucleon-runtime.toml').read_text() != descriptor_text
                or json.loads((target / SHARED / 'frontend.json').read_text()) != contract
                or any(sha(target / relative) != digest for relative, digest in contract['files'].items())):
            raise ValueError(f'Existing ARM64EC generation changed: {target}')
        return target
    with tempfile.TemporaryDirectory(prefix='.prepare-', dir=parent) as temporary:
        stage = Path(temporary) / 'runtime'
        run(['/bin/cp', '-cR', baseline, stage], log=work / 'logs/integration-copy.log')
        # A derived generation must not inherit an install receipt for the baseline.
        (stage / '.nucleon-install.json').unlink(missing_ok=True)
        # Baseline provenance remains available; obsolete graphics are not selected or shipped here.
        old_graphics = tomllib.loads(original)['graphics']
        for key in ['dxmt', 'dxvk', 'vkd3d']:
            if old_graphics.get(key):
                obsolete = stage / old_graphics[key]
                if not obsolete.resolve().is_relative_to(stage.resolve()):
                    raise ValueError(f'Baseline graphics path escapes runtime: {obsolete}')
                if obsolete.is_dir():
                    shutil.rmtree(obsolete)
        for relative, source in copies.items():
            atomic_copy(source, stage / relative)
        if any(sha(stage / relative) != digest for relative, digest in contract['files'].items()):
            raise ValueError('ARM64EC pairing inputs changed during assembly')
        write_json(stage / SHARED / 'frontend.json', contract)
        write_json(stage / SHARED / 'assembly.json', inputs)
        (stage / 'nucleon-runtime.toml').write_text(descriptor_text)
        stage.rename(target)
    return target
