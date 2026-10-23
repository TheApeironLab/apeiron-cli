import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release_signature', Path(__file__).parents[1] / 'scripts/sign-deployment.py')
signer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(signer)


class ReleaseSignature(unittest.TestCase):
    def test_retries_keep_signature_bytes_and_changed_archive_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            home = root / 'gnupg'
            home.mkdir(mode=0o700)
            def gpg(*args):
                return subprocess.check_output(['gpg', '--homedir', str(home), '--batch', *args], stderr=subprocess.DEVNULL)
            try:
                gpg('--faked-system-time', '1767225600!', '--pinentry-mode', 'loopback', '--passphrase', '',
                    '--quick-generate-key', 'Test Release <release@example.invalid>', 'rsa2048', 'sign', '0')
                fingerprint = next(line.split(':')[9] for line in gpg('--with-colons', '--list-keys').decode().splitlines() if line.startswith('fpr:'))
                candidate = {'version': '0.2.1', 'createdAt': '2026-01-02T00:00:00Z'}
                signatures = []
                for attempt in ('first', 'retry'):
                    folder = root / attempt
                    folder.mkdir()
                    archive = folder / 'chentu-0.2.1.tar.gz'
                    archive.write_bytes(b'exact distribution archive')
                    key = 'chentu/releases/0.2.1/chentu-0.2.1.tar.gz'
                    plan = {'version': '0.2.1', 'objects': {key: signer.files.identity(archive)}}
                    (folder / 'upload-plan.json').write_text(json.dumps(plan))
                    signer.sign(folder, home, fingerprint, candidate)
                    signatures.append(Path(str(archive) + '.asc').read_bytes())
                    archive.write_bytes(b'changed bytes')
                    with self.assertRaisesRegex(ValueError, 'changed before signing'):
                        signer.sign(folder, home, fingerprint, candidate)
                self.assertEqual(signatures[0], signatures[1])
                with self.assertRaisesRegex(ValueError, 'prepared candidate'):
                    signer.sign(folder, home, fingerprint, {**candidate, 'version': '0.2.2'})
                with self.assertRaisesRegex(ValueError, 'timezone'):
                    signer.sign(folder, home, fingerprint, {**candidate, 'createdAt': '2026-01-02T00:00:00'})
            finally:
                subprocess.run(['gpgconf', '--homedir', str(home), '--kill', 'all'], check=True)
