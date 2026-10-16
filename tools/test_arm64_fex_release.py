"""Release archive and corresponding-source regression checks; no Wine execution."""
from pathlib import Path
import subprocess
import json
from types import SimpleNamespace
from unittest.mock import patch
import tarfile
import tempfile
import tomllib
import unittest
from arm64_fex_release import archive_files, checked_link, core_descriptor, source_files, version_name, package, build_identity, source_snapshot, RELEASE_FILES, mark_changes, stage_core, sha, REPO, RECIPE, validate_install


class ReleaseTests(unittest.TestCase):
    def git(self, root, *args):
        return subprocess.run(['git', '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null', '-C', str(root), *args], check=True,
                              capture_output=True, text=True).stdout

    def repo(self, root):
        root.mkdir()
        self.git(root, 'init', '-q')
        (root / 'tracked.c').write_text('original\n')
        self.git(root, 'add', 'tracked.c')
        self.git(root, '-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid',
                 'commit', '-qm', 'fixture')
        return root

    def test_modification_notices_are_dated_and_updated_without_duplication(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'module.c').write_text('/* upstream copyright */\nint value;\n')
            (root / 'exports.spec').write_text('@ stdcall exported()\n')
            mark_changes(root, ['module.c', 'exports.spec'], '0.1.0-rc.1', '2026-10-16')
            mark_changes(root, ['module.c', 'exports.spec'], '0.1.0-rc.2', '2026-10-17')
            source = (root / 'module.c').read_text()
            self.assertEqual(source.count('Nucleon runtime downstream modifications:'), 1)
            self.assertIn('2026-10-17, runtime 0.1.0-rc.2.', source)
            self.assertIn('/* upstream copyright */', source)
            self.assertTrue((root / 'exports.spec').read_text().startswith('# Nucleon'))

    def test_tag_validation_prevents_unsafe_asset_names(self):
        self.assertEqual(version_name('runtime-arm64-fex-v0.1.0-rc.1'), '0.1.0-rc.1')
        self.assertEqual(version_name('1.2.3'), '1.2.3')
        for value in ['v1.2.3', '../../bad', '1.0', '1.0.0; echo oops', '1.0.0\n', '-1.2.3']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                version_name(value)

    def test_source_export_uses_patched_bytes_and_intent_added_helper(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.repo(Path(temporary) / 'source')
            (root / 'tracked.c').write_text('patched\n')
            (root / 'helper.h').write_text('borrowed helper\n')
            self.git(root, 'add', '--intent-to-add', 'helper.h')
            (root / 'publisher-secret').write_text('must not ship')
            (root / '.gitignore').write_text('ignored-object\n')
            (root / 'ignored-object').write_text('must not ship')
            files = set(source_files(root))
            self.assertEqual(files, {Path('tracked.c'), Path('helper.h')})
            target = Path(temporary) / 'source.tar.gz'
            archive_files(target, [(root, p, 'source/' + p.as_posix()) for p in files], 123)
            with tarfile.open(target) as archive:
                self.assertEqual(archive.extractfile('source/tracked.c').read(), b'patched\n')
                self.assertEqual(archive.extractfile('source/helper.h').read(), b'borrowed helper\n')

    def test_initialized_submodule_sources_are_included(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            dependency = self.repo(base / 'dependency')
            root = self.repo(base / 'parent')
            self.git(root, '-c', 'protocol.file.allow=always', 'submodule', 'add',
                     str(dependency), 'External/dependency')
            files = set(source_files(root))
            self.assertIn(Path('External/dependency/tracked.c'), files)
            self.assertNotIn(Path('External/dependency/.git'), files)

    def test_archive_is_stable_and_preserves_runtime_modes_and_links(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'wine').write_text('executable\n')
            (root / 'wine').chmod(0o755)
            (root / 'wineboot').symlink_to('wine')
            entries = [(root, Path(name), name) for name in ['wine', 'wineboot']]
            a, b = root / 'a.tar.gz', root / 'b.tar.gz'
            archive_files(a, entries, 123)
            archive_files(b, reversed(entries), 123)
            self.assertEqual(a.read_bytes(), b.read_bytes())
            with tarfile.open(a) as archive:
                self.assertEqual(archive.getmember('wine').mode, 0o755)
                self.assertEqual(archive.getmember('wineboot').linkname, 'wine')
                self.assertEqual(archive.getmember('wine').uid, 0)
                self.assertEqual(archive.getmember('wine').mtime, 123)

    def test_escaping_and_dangling_symlinks_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'root'
            root.mkdir()
            outside = Path(temporary) / 'outside'
            outside.write_text('private')
            for name, target in [('absolute', str(outside)), ('relative', '../outside'), ('dangling', 'missing')]:
                path = root / name
                path.symlink_to(target)
                with self.subTest(name=name), self.assertRaises((ValueError, FileNotFoundError)):
                    checked_link(root, path)

    def test_source_deletion_cannot_silently_drop_corresponding_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.repo(Path(temporary) / 'source')
            (root / 'tracked.c').unlink()
            with self.assertRaisesRegex(ValueError, 'Missing source'):
                list(source_files(root))

    def test_source_snapshot_stays_consistent_if_checkout_changes_during_packaging(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            wine, fex, origin, nucleon = [self.repo(root / name) for name in ['wine', 'fex', 'origin', 'nucleon']]
            revisions = [self.git(p, 'rev-parse', 'HEAD').strip() for p in [wine, fex, origin]]
            sources = root / 'upstream'
            sources.mkdir()
            (sources / 'wine').symlink_to(origin, target_is_directory=True)
            for name in RELEASE_FILES:
                p = nucleon / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text('publisher input')
            experiment = SimpleNamespace(wine=wine, fex=fex, sources=sources,
                recipe={'alternatives': {'tolkara': {'wine': {'revision': revisions[0]},
                                                   'fex': {'revision': revisions[1]}}},
                        'sources': {'wine': {'revision': revisions[2]}}})
            stage = root / 'snapshot'
            stage.mkdir()
            with patch('arm64_fex_release.REPO', nucleon):
                entries = source_snapshot(experiment, stage)
            (wine / 'tracked.c').write_text('changed after snapshot')
            archive_files(root / 'sources.tar.gz', entries, 123)
            with tarfile.open(root / 'sources.tar.gz') as archive:
                self.assertEqual(archive.extractfile('sources/wine/tracked.c').read(), b'original\n')
            manifest = json.loads((stage / 'SOURCE-MANIFEST.json').read_text())
            import hashlib
            self.assertEqual(manifest['wine']['files']['tracked.c']['sha256'],
                             hashlib.sha256(b'original\n').hexdigest())

    def test_packaging_strips_only_pe_images_and_preserves_native_macho_layout(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            recipe = tomllib.loads(RECIPE.read_text())
            runtime = root / 'runtime'
            native = b'\xcf\xfa\xed\xfe' + b'native loader layout'
            host_files = ['bin/wine', 'bin/wineserver', 'lib/wine/aarch64-unix/ntdll.so']
            for relative in host_files:
                p = runtime / relative
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(native)
            receipt = dict(recipe_sha256=sha(RECIPE),
                patches={p: sha(REPO / p) for p in recipe['wine']['tolkara']['patches']},
                host_files={p: sha(runtime / p) for p in host_files})
            (runtime / 'build.json').write_text(json.dumps(receipt))
            for directory, name in [('arm64ec', 'libarm64ecfex.dll'), ('wow64', 'libwow64fex.dll')]:
                p = root / 'fex' / directory / 'Bin' / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(b'MZ Windows guest')
            unix = root / 'fex/unixlib'
            unix.mkdir()
            for name in ['libarm64ecfex.so', 'libwow64fex.so']:
                (unix / name).write_bytes(native)
            (unix / 'build.json').write_text(json.dumps(dict(files={p.name: sha(p) for p in unix.glob('*.so')})))
            compiler = root / 'compiler'
            compiler.mkdir()
            (compiler / 'LICENSE.TXT').write_text('compiler notices')
            for triple in ['arm64ec', 'aarch64', 'i686']:
                path = compiler / f'{triple}-w64-mingw32/share/mingw32'
                path.mkdir(parents=True)
                (path / 'COPYING').write_text('runtime notices')
            destination = root / 'package'
            with patch('arm64_fex_release.run') as command:
                stage_core(SimpleNamespace(work=root, recipe=recipe, compiler=compiler), destination)
            strip_calls = [c.args[0] for c in command.call_args_list if '--strip-debug' in c.args[0]]
            self.assertTrue(strip_calls)
            for call in strip_calls:
                self.assertTrue(all(Path(path).read_bytes().startswith(b'MZ') for path in call[2:]))
            self.assertEqual((destination / 'bin/wine').read_bytes(), native)
            self.assertTrue((destination / 'share/licenses/toolchain/LLVM-LICENSE.TXT').is_file())

    def test_every_guest_dll_is_bound_to_the_publisher_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            experiment = SimpleNamespace(work=root)
            files = ['runtime/bin/wine', 'runtime/lib/wine/i386-windows/user32.dll',
                     'fex/arm64ec/Bin/libarm64ecfex.dll', 'fex/wow64/Bin/libwow64fex.dll',
                     'fex/unixlib/libarm64ecfex.so', 'fex/unixlib/libwow64fex.so']
            for name in files:
                p = root / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(b'original')
            manifest = root / 'sources.json'
            manifest.write_text(json.dumps({'wine': {'files': {}}, 'nucleon': {'files': {}}}))
            before = build_identity(experiment, manifest)
            (root / files[1]).write_bytes(b'changed')
            self.assertNotEqual(before, build_identity(experiment, manifest))

    def test_cleanup_failure_preserves_the_primary_startup_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            support, logs = root / 'support', root / 'logs'
            runtime = support / 'runtimes/arm64-fex/generation'
            runtime.mkdir(parents=True)
            logs.mkdir()
            (runtime / '.nucleon-install.json').write_text('{}')
            (runtime / 'nucleon-runtime.toml').write_text(core_descriptor())
            archive = root / 'runtime.tar.gz'
            archive.write_bytes(b'archive fixture')
            def command(argv, **_kwargs):
                if 'wineboot' in argv:
                    raise RuntimeError('primary boot failure')
                if '-w' in argv:
                    raise RuntimeError('secondary cleanup failure')
            experiment = SimpleNamespace(compiler=root / 'compiler', env={})
            with patch('arm64_fex_release.run', side_effect=command), \
                 patch('arm64_fex_release.subprocess.run', return_value=SimpleNamespace(returncode=1)):
                with self.assertRaisesRegex(RuntimeError, 'primary boot failure'):
                    validate_install(root / 'nucleon', archive, support, experiment, logs)
            self.assertIn('secondary cleanup failure', (logs / 'cleanup.log').read_text())

    def test_validation_failure_never_publishes_partial_assets(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            experiment = SimpleNamespace(work=root, recipe={'toolchain': {}})
            (root / 'publisher-build.json').write_text('{}')
            def stage(_experiment, destination):
                destination.mkdir()
                (destination / 'nucleon-runtime.toml').write_text(core_descriptor())
            def snapshot(_experiment, destination):
                (destination / 'SOURCE-MANIFEST.json').write_text('{}')
                return [(destination, Path('SOURCE-MANIFEST.json'), 'SOURCE-MANIFEST.json')]
            with patch('arm64_fex_release.stage_core', side_effect=stage), \
                 patch('arm64_fex_release.source_snapshot', side_effect=snapshot), \
                 patch('arm64_fex_release.build_identity', return_value={}), \
                 patch('arm64_fex_release.output', side_effect=['123', '15.0']), \
                 patch('arm64_fex_release.validate_install', side_effect=ValueError('guest failed')):
                with self.assertRaisesRegex(ValueError, 'guest failed'):
                    package(experiment, '0.1.0-rc.1', root / 'assets', root / 'nucleon')
            self.assertFalse((root / 'assets').exists())
            self.assertTrue(list((root / 'release-validation').iterdir()))

    def test_core_contract_declares_both_guest_companions_without_graphics_bundles(self):
        descriptor = tomllib.loads(core_descriptor())
        self.assertEqual(descriptor['guests'], ['x64', 'x86'])
        self.assertEqual({e['guest'] for e in descriptor['emulators']}, {'x64', 'x86'})
        self.assertTrue(all(e['unix_library'].endswith('.so') for e in descriptor['emulators']))
        self.assertNotIn('graphics', descriptor)
        self.assertEqual(descriptor['backends'], ['auto'])


if __name__ == '__main__':
    unittest.main()
