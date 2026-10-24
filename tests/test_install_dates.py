import gzip
from pathlib import Path
import tempfile
import unittest
from application_remover.backend import debian_install_dates


class InstallDateTests(unittest.TestCase):
    def test_rotated_logs_updates_reinstall_and_architectures(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with gzip.open(root / 'dpkg.log.1.gz', 'wt') as log:
                log.write('2025-01-01 10:00:00 install demo:amd64 <none> 1\n'
                          '2025-01-01 10:00:00 install stable:amd64 <none> 1\n'
                          '2025-01-01 10:00:00 install gone:amd64 <none> 1\n'
                          '2025-01-01 10:00:00 install multi:amd64 <none> 1\n'
                          '2025-01-02 10:00:00 install multi:i386 <none> 1\n')
            (root / 'dpkg.log').write_text(
                '2025-02-01 10:00:00 upgrade stable:amd64 1 2\n'
                '2025-02-01 10:00:00 remove gone:amd64 1 <none>\n'
                '2025-02-01 10:00:00 remove demo:amd64 1 <none>\n'
                '2025-03-01 10:00:00 install demo:amd64 <none> 2\n'
                'bad-date bad-time install invalid:amd64 <none> 1\n')
            dates = debian_install_dates(directory)
            self.assertEqual(dates['demo'], '2025-03-01 10:00:00')
            self.assertEqual(dates['stable'], '2025-01-01 10:00:00')
            self.assertNotIn('gone:amd64', dates)
            self.assertNotIn('multi', dates)
            self.assertIn('multi:i386', dates)
            self.assertNotIn('invalid:amd64', dates)

    def test_missing_history(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(debian_install_dates(directory), {})
