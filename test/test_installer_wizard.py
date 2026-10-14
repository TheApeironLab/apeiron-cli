"""Pure checks for the isolated wizard smoke runner; no server or deployment."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    'installer_wizard', Path(__file__).resolve().parents[1] / 'scripts/test-installer-wizard.py')
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class PayloadBindingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.package = Path(self.temp.name)
        self.bundle = self.package / 'bundle'
        self.bundle.mkdir()
        self.file = self.bundle / 'payload'
        self.file.write_bytes(b'original resource')
        self.plan = {'files': [{'path': 'payload', 'size': self.file.stat().st_size,
                               'sha256': smoke.sha(self.file)}]}

    def test_accepts_exact_contents(self):
        smoke.verify_payload(self.plan, self.package)

    def test_rejects_same_size_changed_contents(self):
        self.file.write_bytes(b'changed resource!')
        with self.assertRaisesRegex(ValueError, 'does not match'):
            smoke.verify_payload(self.plan, self.package)

    def test_rejects_missing_resource(self):
        self.file.unlink()
        with self.assertRaisesRegex(ValueError, 'Missing payload'):
            smoke.verify_payload(self.plan, self.package)

    def test_rejects_undeclared_resource(self):
        (self.bundle / 'unexpected').write_text('extra')
        with self.assertRaisesRegex(ValueError, 'does not match'):
            smoke.verify_payload(self.plan, self.package)

    def test_rejects_symlink(self):
        self.file.unlink()
        self.file.symlink_to('/etc/hosts')
        with self.assertRaisesRegex(ValueError, 'Non-regular'):
            smoke.verify_payload(self.plan, self.package)

    def test_hash_is_python310_compatible(self):
        self.file.write_bytes(b'abc')
        self.assertEqual(smoke.sha(self.file),
                         'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad')


if __name__ == '__main__':
    unittest.main()
