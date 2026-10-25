# secrets: none
"""Minimal online material ownership; no network or cluster operations."""
import copy
import hashlib
import io
import json
import tarfile
import tempfile
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('distro_material', Path(__file__).parents[1] / 'scripts/prepare-distro-material.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class MinimalMaterial(unittest.TestCase):
    def test_only_native_inventory_owners_supply_material(self):
        catalog = {'components': {
            'apeiron': {'requires': ['gateway'], 'files': ['app.tar', 'skills.bundle'],
                        'images': [{'file': 'app.tar', 'reference': 'apeiron/app:1', 'digest': 'sha256:abc'}]},
            'ingress': {'files': ['cert.tgz']},
            'gateway': {'files': ['gateway.tar']},
        }}
        before = copy.deepcopy(catalog)
        receipts = module.selected_receipts(catalog, {'releases': [{'name': 'apeiron', 'needs': []}, {'name': 'cert-manager'}]})
        self.assertEqual(receipts, [
            {'components': ['apeiron'], 'file': 'app.tar', 'reference': 'apeiron/app:1', 'digest': 'sha256:abc'},
            {'components': ['apeiron'], 'files': ['skills.bundle']},
            {'components': ['ingress'], 'files': ['cert.tgz']},
        ])
        self.assertEqual(catalog, before)

    def test_missing_owner_or_unowned_image_is_rejected(self):
        inventory = {'releases': [{'name': 'apeiron'}]}
        with self.assertRaises(KeyError):
            module.selected_receipts({'components': {}}, inventory)
        with self.assertRaisesRegex(ValueError, 'ownership'):
            module.selected_receipts({'components': {'apeiron': {'files': [], 'images': [{'file': 'app.tar'}]}}}, inventory)

    def test_local_client_closure_and_provenance_are_preserved(self):
        local = {'apeironVersion': 'v0.2.1', 'scodeVersion': 'v0.2.33'}
        image = {'file': 'app.tar', 'reference': 'apeiron/app:1', 'digest': 'sha256:abc', 'buildProvenance': {'source': {'commit': 'a' * 40}}}
        catalog = {'components': {'apeiron': {
            'files': ['app.tar', 'local-computer/cli.tar', 'charts/apeiron.tgz'], 'images': [image],
            'values': {'releases': {'apeiron': {'values': {'localComputer': local}}}},
        }}}
        receipts = module.selected_receipts(catalog, {'releases': [{'name': 'apeiron'}]})
        self.assertEqual(receipts[0], {'components': ['apeiron'], **image})
        self.assertEqual(receipts[1], {'components': ['apeiron'], 'files': ['local-computer/cli.tar'], 'localComputer': local})
        self.assertEqual(receipts[2]['files'], ['charts/apeiron.tgz'])

    def test_cli_requires_matching_bytes_and_embedded_runtime(self):
        binary = b'fixture executable'
        metadata = {'kind': 'apeiron-cli-build', 'version': '0.2.1', 'target': 'linux-arm64',
                    'source': {'revision': 'a' * 40}, 'sudocode': {'version': 'v0.2.33'},
                    'binary': {'size': len(binary), 'sha256': hashlib.sha256(binary).hexdigest()}}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'apeiron.tar.gz'
            with tarfile.open(path, 'w:gz') as archive:
                for name, data in [('README.md', b'development fixture'), ('apeiron', binary),
                                   ('build-provenance.json', json.dumps(metadata).encode())]:
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    archive.addfile(info, io.BytesIO(data))
            release = {'revision': 'a' * 40, 'version': '0.2.1', 'builds': {'linux-arm64': metadata},
                       'files': {path.name: {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                                            'size': path.stat().st_size}}}
            self.assertEqual(module.verify_cli_archive(path, release), metadata)
            wrong = copy.deepcopy(release)
            wrong['builds']['linux-arm64'].pop('sudocode')
            with self.assertRaisesRegex(ValueError, 'provenance'):
                module.verify_cli_archive(path, wrong)
            path.write_bytes(path.read_bytes() + b'changed')
            with self.assertRaisesRegex(ValueError, 'differs'):
                module.verify_cli_archive(path, release)


if __name__ == '__main__':
    unittest.main()
