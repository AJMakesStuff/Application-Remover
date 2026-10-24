import unittest
from unittest.mock import patch
from application_remover import backend as b


class BackendTests(unittest.TestCase):
    def test_inventory_includes_local_packages_and_excludes_removed(self):
        rows = ('local-app\tinstalled\t1.0\t12\tno\tno\tutils\tLocal app\n'
                'old\tconfig-files\t1\t0\tno\tno\tutils\tOld\n'
                'bash\tinstalled\t5\t100\tyes\tno\tshells\tShell\n')
        packages = b.parse_debian(rows)
        self.assertEqual([p.name for p in packages], ['local-app', 'bash'])
        self.assertEqual(packages[0].size, 12288)
        self.assertTrue(packages[1].protected)

    @patch.object(b, 'run', side_effect=['', 'Remv demo [1]\nRemv dependent [2]\n'])
    def test_plan_includes_dependents(self, run):
        self.assertEqual(b.removal_plan(b.Package('demo', '1', 'Debian'), []).names, ('demo', 'dependent'))

    @patch.object(b, 'run', return_value='Remv bash [5]\n')
    def test_blocks_protected_dependency(self, run):
        with self.assertRaisesRegex(RuntimeError, 'protected'):
            b.removal_plan(b.Package('demo', '1', 'Debian'), [b.Package('bash', '5', 'Debian', protected=True)])

    @patch.object(b, 'run')
    def test_blocks_direct_protected_removal(self, run):
        with self.assertRaises(RuntimeError):
            b.removal_plan(b.Package('bash', '5', 'Debian', protected=True), [])
        run.assert_not_called()

    @patch.object(b, 'run', return_value='Remv demo [1]\nInst other [2]\n')
    def test_blocks_mixed_transaction(self, run):
        with self.assertRaises(RuntimeError):
            b.removal_plan(b.Package('demo', '1', 'Debian'), [])

    @patch.object(b, 'run', return_value='Remv demo [1]\nRemv new-dependent [1]\n')
    def test_changed_plan_never_executes(self, run):
        with self.assertRaisesRegex(RuntimeError, 'changed'):
            b.remove(b.Package('demo', '1', 'Debian'), [], ['demo'])
        self.assertEqual(run.call_count, 2)
        self.assertIn('--simulate', run.call_args.args[0])

    def test_flatpak_uses_exact_ref_and_scope(self):
        p = b.Package('Demo', '1', 'Flatpak', ref='org.demo.App/x86_64/stable', scope='user')
        self.assertEqual(b.removal_command(p), ['/usr/bin/flatpak', 'uninstall', '--noninteractive', '--user', p.ref])

    def test_apt_requires_dependency_plan(self):
        with self.assertRaisesRegex(ValueError, 'reviewed dependency plan'):
            b.removal_command(b.Package('demo:amd64', '1', 'Debian'))


if __name__ == '__main__':
    unittest.main()
