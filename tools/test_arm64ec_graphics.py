"""Frontend policy tests; never run Wine or a graphics compiler."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from arm64ec_graphics import Frontends, cross_file, image_identity


class FrontendTests(unittest.TestCase):
    def test_only_the_pinned_source_overlay_is_accepted(self):
        from arm64_fex import REPO, sha
        with tempfile.TemporaryDirectory() as temporary:
            experiment = Frontends.__new__(Frontends)
            experiment.work = Path(temporary)
            revision = 'a' * 40
            patch_name = 'assets/patches/dxmt-initialize-device-before-command-helpers.patch'
            experiment.pin = {'sources':{'dxmt':{'revision':revision}}, 'dxmt':{'patches':[patch_name]}}
            source = experiment.source('dxmt')
            source.mkdir(parents=True)
            marker = source / '.nucleon-overlay.json'
            marker.write_text(json.dumps({'diff_sha256':'approved', 'patches':{patch_name:sha(REPO / patch_name)}}))
            with patch('arm64ec_graphics.output', side_effect=[revision, ' M source.cpp', '']), patch('arm64ec_graphics.source_diff', return_value='approved'):
                self.assertEqual(experiment.verify_source('dxmt'), '')
            with patch('arm64ec_graphics.output', side_effect=[revision, ' M source.cpp']), patch('arm64ec_graphics.source_diff', return_value='changed'):
                with self.assertRaisesRegex(ValueError, 'Modified source'):
                    experiment.verify_source('dxmt')

    def test_component_identity_reads_valid_pe64_and_rejects_truncated_inputs(self):
        import struct
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'frontend.dll'
            data = bytearray(512)
            data[:2] = b'MZ'
            struct.pack_into('<I', data, 60, 128)
            data[128:132] = b'PE\0\0'
            struct.pack_into('<I', data, 136, 123)
            struct.pack_into('<H', data, 152, 0x20b)
            struct.pack_into('<I', data, 208, 4096)
            struct.pack_into('<I', data, 216, 456)
            path.write_bytes(data)
            self.assertEqual(image_identity(path), '0000007b:00001000:000001c8')
            for broken in [b'', data[:200]]:
                path.write_bytes(broken)
                with self.assertRaisesRegex(ValueError, 'Invalid frontend'):
                    image_identity(path)

    def test_homebrew_opt_alias_is_preserved_for_dxmt_static_dependencies(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cellar = root / 'Cellar/llvm@15/15.0.7'
            cellar.mkdir(parents=True)
            alias = root / 'opt/llvm@15'
            alias.parent.mkdir()
            alias.symlink_to(cellar, target_is_directory=True)
            base = SimpleNamespace(env={'PATH':'/usr/bin:/bin'}, base=SimpleNamespace(compiler=root / 'compiler'))
            with patch('arm64ec_graphics.DX12', return_value=base):
                experiment = Frontends(root / 'work', root / 'sdk', 1, native_llvm=alias)
            self.assertEqual(experiment.native_llvm, alias.absolute())
            self.assertNotEqual(experiment.native_llvm, cellar)

    def test_dxmt_prerequisite_errors_explain_both_repairs_before_building(self):
        experiment = Frontends.__new__(Frontends)
        experiment.native_llvm = Path('/missing/llvm')
        experiment.env = {}
        experiment.pin = {'dxmt':{'native_llvm_version':'15.0.7'}}
        with patch('arm64ec_graphics.output', side_effect=FileNotFoundError('missing tool')):
            with self.assertRaises(ValueError) as failure:
                experiment.dxmt_prerequisites()
        self.assertIn('brew install llvm@15', str(failure.exception))
        self.assertIn('xcodebuild -downloadComponent MetalToolchain', str(failure.exception))
        with patch('arm64ec_graphics.output', side_effect=['22.1.0', 'Metal compiler']):
            with self.assertRaisesRegex(ValueError, 'Unexpected LLVM version'):
                experiment.dxmt_prerequisites()
        with patch('arm64ec_graphics.output', side_effect=['15.0.7', 'Metal compiler']):
            self.assertEqual(experiment.dxmt_prerequisites()['native_llvm'], '15.0.7')

    def test_cross_files_use_explicit_target_tools_and_do_not_run_pe_programs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'bin').mkdir()
            for triple in ('arm64ec', 'x86_64'):
                for suffix in ('gcc', 'g++', 'ar', 'strip', 'windres', 'widl'):
                    (root / 'bin' / f'{triple}-w64-mingw32-{suffix}').touch()
            native = cross_file(root, 'arm64ec')
            control = cross_file(root, 'x64')
            self.assertIn("cpu_family = 'aarch64'", native)
            self.assertIn('arm64ec-w64-mingw32-g++', native)
            self.assertNotIn('x86_64-w64', native)
            self.assertIn('needs_exe_wrapper = true', native)
            self.assertIn("cpu_family = 'x86_64'", control)
            self.assertNotIn('arm64ec-w64', control)
            with self.assertRaises(ValueError): cross_file(root, 'arm64')
            (root / 'bin/arm64ec-w64-mingw32-ar').unlink()
            with self.assertRaisesRegex(ValueError, 'Missing compiler'): cross_file(root, 'arm64ec')

    def test_modified_bundle_is_rejected_before_starting_wine(self):
        with tempfile.TemporaryDirectory() as temporary:
            experiment = Frontends.__new__(Frontends)
            experiment.work = Path(temporary)
            bundle = experiment.work / 'bundles/arm64ec/dxvk'
            bundle.mkdir(parents=True)
            (bundle / 'dxgi.dll').write_bytes(b'changed')
            (bundle / 'build.json').write_text(json.dumps({'architecture':'arm64ec',
                'status':'BUILT_NOT_EXECUTION_VALIDATED', 'files':{'dxgi.dll':'wrong-digest'}}))
            with patch('arm64ec_graphics.run') as execute:
                with self.assertRaisesRegex(ValueError, 'Modified frontend'):
                    experiment.probe('arm64ec', 'dx12')
                execute.assert_not_called()

    def test_mismatched_darwin_thread_pointer_contract_fails_before_compilation(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            experiment = Frontends.__new__(Frontends)
            experiment.pin = {'darwin':{'teb_tsd_offset':'0x17f8'}}
            experiment.base = SimpleNamespace(work=root, base=SimpleNamespace(work=root))
            (root / 'wine-build').mkdir()
            (root / 'wine-build/Makefile').write_text('-D__WINE_TEB_TSD_OFFSET=0x17f8 ')
            (root / 'fex/arm64ec').mkdir(parents=True)
            (root / 'fex/arm64ec/CMakeCache.txt').write_text('FEX_TEB_TSD_OFFSET:STRING=0x999\n')
            with patch.object(experiment, 'verify_source'), patch('arm64ec_graphics.run') as execute:
                with self.assertRaisesRegex(ValueError, 'thread-pointer ABI'):
                    experiment.darwin_startup()
                execute.assert_not_called()


if __name__ == '__main__': unittest.main()
