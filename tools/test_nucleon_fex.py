"""Exercise diagnostic wiring with a fake runner; never executes Wine or games."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from nucleon_fex import configuration_lock, installed_runtime, launch


class DiagnosticLaunchTests(unittest.TestCase):
    def test_installed_selection_uses_managed_runtime_without_sdk_or_build_tree(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            root = home / 'managed-runtime'
            root.mkdir()
            (root / '.nucleon-install.json').write_text('{}')
            (root / 'nucleon-runtime.toml').write_text('schema=1')
            selection = home / 'Library/Application Support/Steam/compatibilitytools.d/nucleon-fex/fex-runtime.json'
            selection.parent.mkdir(parents=True)
            selection.write_text(json.dumps(dict(root=str(root))))
            with patch('pathlib.Path.home', return_value=home):
                self.assertEqual(installed_runtime(),root.resolve())
                (root / 'nucleon-runtime.toml').unlink()
                with self.assertRaisesRegex(ValueError,'incomplete'):
                    installed_runtime()

    def test_runner_receives_private_profile_and_clean_environment(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runtime = root / 'runtime'
            runtime.mkdir()
            (runtime / 'nucleon-runtime.toml').write_text('[prefix]\nfamily="fixture"\nabi=1\n')
            game = root / 'game.exe'
            game.touch()
            runner = root / 'runner'
            capture = root / 'capture.json'
            runner.write_text(f'#!{sys.executable}\n' + '''import json, os, sys
from pathlib import Path
selection = Path(os.environ['NUCLEON_FEX_SELECTION'])
profile = Path(os.environ['NUCLEON_GAME_PROFILE'])
Path(os.environ['NUCLEON_SUPPORT_DIR']).joinpath('capture.json').write_text(json.dumps(dict(
    argv=sys.argv[1:], environment=dict(os.environ), selection=json.loads(selection.read_text()),
    profile=json.loads(profile.read_text()))))
Path(os.environ['NUCLEON_RUN_RECEIPT']).write_text(json.dumps(dict(status='bounded',timings=dict(preparation_ms=25,runtime_ms=7000,cleanup_ms=50))))
print('quiet runner')
''')
            runner.chmod(0o755)
            profile = dict(graphics_api='dx12', steam_protocol='valve-redirect', overlay=False)
            with patch.dict(os.environ, {'NUCLEON_RUNNER':str(runner), 'NUCLEON_SUPPORT_DIR':str(root),
                    'WINEPREFIX':'unrelated', 'VK_DRIVER_FILES':'stale', 'NUCLEON_ENGINE':'gptk'}):
                result = launch(runtime, root / 'bridge', 601150, game, root / 'compat',
                                profile, root / 'launch.log', 7)
            captured = json.loads(capture.read_text())
            self.assertEqual(captured['argv'], ['run', str(game)])
            self.assertEqual(captured['profile'], profile)
            self.assertEqual(captured['selection']['root'], str(runtime.resolve()))
            env = captured['environment']
            for name in ['WINEPREFIX', 'VK_DRIVER_FILES', 'NUCLEON_ENGINE']:
                self.assertNotIn(name, env)
            self.assertEqual(env['NUCLEON_RUN_SECONDS'], '7')
            self.assertEqual(env['NUCLEON_DEBUG'], '0')
            self.assertNotIn('NUCLEON_TRACE_PROCESS_EXIT',env)
            self.assertEqual(result['timings']['runtime_ms'],7000)
            self.assertFalse(Path(env['NUCLEON_GAME_PROFILE']).exists())
            self.assertEqual(result['status'], 'BOUNDED_RUN_FINISHED')
            self.assertTrue(result['prefix'].endswith('/compat/nucleon/arm64-fex/fixture/pfx'))
            with patch.dict(os.environ, {'NUCLEON_RUNNER':str(runner), 'NUCLEON_SUPPORT_DIR':str(root)}):
                launch(runtime, root / 'bridge', 601150, game, root / 'compat',
                       profile, root / 'debug.log', 7, debug=True)
            self.assertEqual(json.loads(capture.read_text())['environment']['NUCLEON_DEBUG'], '1')

    def test_configuration_lock_covers_the_actual_path_and_releases_after_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'game.ini'
            path.write_bytes(b'original\r\n')
            script = 'from pathlib import Path; from nucleon_fex import configuration_lock; '\
                     'lock=configuration_lock(Path(__import__("sys").argv[1])); lock.__enter__()'
            with self.assertRaises(ValueError):
                with configuration_lock(path):
                    result = subprocess.run([sys.executable, '-c', script, str(path)],
                        cwd=Path(__file__).parent, capture_output=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(b'Another diagnostic launch', result.stderr)
                    raise ValueError('simulated setup failure')
            with configuration_lock(path):
                self.assertEqual(path.read_bytes(), b'original\r\n')


if __name__ == '__main__':
    unittest.main()
