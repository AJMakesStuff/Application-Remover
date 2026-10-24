"""Exercise the real APT solver against an isolated, synthetic package database."""
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch
from application_remover import backend as b


@unittest.skipUnless(shutil.which('apt-get'), 'APT is required')
class DependencyCleanupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        # Includes a recursive private dependency, a shared dependency, a manual
        # library, and an unrelated orphan with its own required dependency.
        graph = {
            'demo': 'private-lib, shared-lib, manual-lib',
            'other-app': 'shared-lib',
            'private-lib': 'leaf-lib',
            'leaf-lib': '', 'shared-lib': '', 'manual-lib': '',
            'old-orphan': 'old-leaf', 'old-leaf': '',
        }
        (root / 'status').write_text(''.join(
            f'Package: {name}\nStatus: install ok installed\nPriority: optional\n'
            f'Architecture: amd64\nVersion: 1.0\nMaintainer: Test <test@example.com>\n'
            + (f'Depends: {deps}\n' if deps else '') + 'Description: test fixture\n\n'
            for name, deps in graph.items()))
        (root / 'extended_states').write_text(''.join(
            f'Package: {name}\nArchitecture: amd64\nAuto-Installed: 1\n\n'
            for name in ('private-lib', 'leaf-lib', 'shared-lib', 'old-orphan', 'old-leaf')))
        (root / 'lists/partial').mkdir(parents=True)
        (root / 'archives/partial').mkdir(parents=True)
        self.options = ['-o', f'Dir::State::status={root}/status',
                        '-o', f'Dir::State::extended_states={root}/extended_states',
                        '-o', f'Dir::State::lists={root}/lists',
                        '-o', f'Dir::Cache::archives={root}/archives',
                        '-o', 'Dir::Etc::sourcelist=/dev/null',
                        '-o', 'Dir::Etc::sourceparts=-',
                        '-o', 'APT::Architecture=amd64',
                        '-o', 'Debug::NoLocking=1']
        self.real_run = b.run
        self.package = b.Package('demo', '1.0', 'Debian')

    def simulate(self, args, **kwargs):
        self.assertIn('--simulate', args, 'Test must never perform a real removal')
        return self.real_run([args[0], *self.options, *args[1:]], **kwargs)

    def test_only_newly_unused_dependencies_removed(self):
        with patch.object(b, 'run', side_effect=self.simulate):
            plan = b.removal_plan(self.package, [])
        self.assertEqual(set(plan.names), {'demo', 'private-lib', 'leaf-lib'})
        self.assertIn('--autoremove', plan.command)
        self.assertIn(r'APT::NeverAutoRemove::=^old\-orphan$', plan.command)
        # Preview and actual command use precisely the same solver options.
        simulated = self.simulate(['apt-get', '--simulate', *plan.command[3:]])
        self.assertEqual(set(b.simulated_removals(simulated)), set(plan.names))

    def test_recheck_and_execution_use_approved_command(self):
        with patch.object(b, 'run', side_effect=self.simulate):
            plan = b.removal_plan(self.package, [])
        with patch.object(b, 'removal_plan', return_value=plan), patch.object(b, 'run', return_value='ok') as run:
            b.remove(self.package, [], plan)
        run.assert_called_once_with(list(plan.command), timeout=None)
