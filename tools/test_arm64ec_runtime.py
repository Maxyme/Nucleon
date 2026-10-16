"""No-execution coverage for isolated frontend generation assembly."""
import json
from pathlib import Path
import shutil
import tempfile
import tomllib
import unittest
from unittest.mock import patch

from arm64_fex import REPO, sha
from arm64ec_runtime import COMPONENTS, SHARED, assemble, experimental_descriptor, paired_inputs


class AssemblyTests(unittest.TestCase):
    def fixture(self, root):
        work = root / 'arm64ec'
        baseline = root / 'baseline'
        recipe = REPO / 'assets/wine/recipes/arm64ec-graphics.toml'
        pin = tomllib.loads(recipe.read_text())
        original = '''schema=1
mode='arm64-fex'
guests=['x64','x86']
loader='bin/wine'
server='bin/wineserver'
unix_dir='lib/wine/aarch64-unix'
windows_dirs=['lib/wine/aarch64-windows','lib/wine/i386-windows']
backends=['auto','dxmt','dxvk','kosmickrisp']
steam_helper=true
steam_protocol='valve-redirect'
[prefix]
family='fixture'
abi=1
[[emulators]]
guest='x64'
path='lib/wine/aarch64-windows/libarm64ecfex.dll'
unix_library='lib/wine/aarch64-unix/libarm64ecfex.so'
[[emulators]]
guest='x86'
path='lib/wine/aarch64-windows/libwow64fex.dll'
unix_library='lib/wine/aarch64-unix/libwow64fex.so'
[graphics]
dxmt='share/nucleon/dxmt'
dxvk='share/nucleon/graphics'
vkd3d='share/nucleon/graphics'
vulkan_icd='lib/vulkan/icd.json'
vulkan_loader='lib/vulkan/loader.dylib'
'''
        files = ['bin/wine', 'bin/wineserver', 'lib/wine/aarch64-unix/ntdll.so',
                 'lib/wine/aarch64-unix/winevulkan.so', 'lib/wine/aarch64-unix/libarm64ecfex.so',
                 'lib/wine/aarch64-windows/ntdll.dll', 'lib/wine/aarch64-windows/winevulkan.dll',
                 'lib/wine/aarch64-windows/libarm64ecfex.dll', 'lib/vulkan/driver.dylib',
                 'lib/vulkan/loader.dylib', 'share/nucleon/dxmt/d3d11.dll',
                 'share/nucleon/graphics/d3d12.dll']
        for name in files:
            path = baseline / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(name.encode())
        (baseline / 'nucleon-runtime.toml').write_text(original)
        (baseline / 'lib/vulkan/icd.json').write_text(json.dumps({'ICD': {'library_path': 'driver.dylib'}}))
        (work / 'crt').mkdir(parents=True)
        for name in ['dllcrt2.o', 'emutls.o']:
            (work / 'crt' / name).write_bytes(name.encode())
        for component, names in COMPONENTS.items():
            bundle = work / 'bundles/arm64ec' / component
            bundle.mkdir(parents=True)
            for name in names:
                (bundle / name).write_bytes(('arm64ec-' + name).encode())
            build = dict(architecture='arm64ec', status='BUILT_NOT_EXECUTION_VALIDATED',
                         source=pin['sources'][component], mingw_source=pin['sources']['mingw'],
                         recipe_sha256=sha(recipe), toolchain={'version': 'fixture'},
                         options=[f'{prefix}-femulated-tls -DNUCLEON_TEB_TSD_OFFSET={pin["darwin"]["teb_tsd_offset"]}'
                                  for prefix in ['-Dc_args=', '-Dcpp_args=']],
                         crt_sha256=sha(work / 'crt/dllcrt2.o'), emutls_sha256=sha(work / 'crt/emutls.o'),
                         teb_header_sha256=sha(REPO / 'tools/runtime-probes/darwin-teb.h'),
                         files={name: sha(bundle / name) for name in names})
            (bundle / 'build.json').write_text(json.dumps(build))
        return baseline, work, pin

    def copy_command(self, command, **kwargs):
        self.assertEqual(command[:2], ['/bin/cp', '-cR'])
        shutil.copytree(command[2], command[3])

    def test_assembly_is_separate_reusable_and_records_complete_current_pair(self):
        with tempfile.TemporaryDirectory() as temporary:
            baseline, work, pin = self.fixture(Path(temporary))
            original = (baseline / 'nucleon-runtime.toml').read_bytes()
            with patch('arm64ec_runtime.run', side_effect=self.copy_command) as execute:
                target = assemble(baseline, work, pin)
                inode = (target / 'nucleon-runtime.toml').stat().st_ino
                self.assertEqual(assemble(baseline, work, pin), target)
                self.assertEqual(execute.call_count, 1)
            descriptor = tomllib.loads((target / 'nucleon-runtime.toml').read_text())
            self.assertEqual(descriptor['graphics']['frontend_abi'], 'arm64ec')
            self.assertEqual(descriptor['guests'], ['x64'])
            self.assertEqual(descriptor['prefix']['family'], 'fixture-arm64ec')
            self.assertEqual(descriptor['prefix']['abi'], 1)
            self.assertEqual(len(descriptor['emulators']), 1)
            self.assertNotIn('dxmt', descriptor['graphics'])
            self.assertFalse((target / 'share/nucleon/dxmt').exists())
            self.assertEqual((baseline / 'nucleon-runtime.toml').read_bytes(), original)
            self.assertEqual((target / 'nucleon-runtime.toml').stat().st_ino, inode)
            contract = json.loads((target / SHARED / 'frontend.json').read_text())
            self.assertIn('lib/wine/aarch64-unix/libarm64ecfex.so', contract['files'])
            for name, digest in contract['files'].items():
                self.assertEqual(sha(target / name), digest)
            report = json.loads((target / SHARED / 'assembly.json').read_text())
            self.assertEqual(report['steam_validation'], 'NOT_RUN')
            self.assertEqual(report['validation'], 'NOT_EXECUTION_VALIDATED')
            damaged = target / SHARED / 'dxvk/dxgi.dll'
            damaged.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'generation changed'):
                assemble(baseline, work, pin)
            self.assertEqual(damaged.read_bytes(), b'changed')

    def test_modified_frontend_or_crt_cannot_publish_a_generation(self):
        for name in ['bundles/arm64ec/dxvk/dxgi.dll', 'crt/emutls.o']:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                baseline, work, pin = self.fixture(Path(temporary))
                (work / name).write_bytes(b'changed')
                with patch('arm64ec_runtime.run') as execute, self.assertRaises(ValueError):
                    assemble(baseline, work, pin)
                execute.assert_not_called()
                self.assertFalse((work.parent / 'arm64ec-integrated').exists())

    def test_unpaired_source_receipt_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            _baseline, work, pin = self.fixture(Path(temporary))
            receipt = work / 'bundles/arm64ec/vkd3d/build.json'
            build = json.loads(receipt.read_text())
            build['source']['revision'] = 'unknown'
            receipt.write_text(json.dumps(build))
            with self.assertRaisesRegex(ValueError, 'build provenance'):
                paired_inputs(work, pin)

    def test_descriptor_rejects_unsupported_baselines_without_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            baseline, _work, _pin = self.fixture(Path(temporary))
            original = (baseline / 'nucleon-runtime.toml').read_text()
            for text in [original.replace("mode='arm64-fex'", "mode='intel'"),
                         original.replace("vulkan_loader='lib/vulkan/loader.dylib'", ''),
                         original.replace("family='fixture'", "family='" + 'x' * 64 + "'")]:
                with self.assertRaises(ValueError):
                    experimental_descriptor(text)


if __name__ == '__main__':
    unittest.main()
