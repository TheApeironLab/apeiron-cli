# secrets: none
"""Development preview integrity and publication boundary; no external requests."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('distro_preview', Path(__file__).parents[1] / 'scripts/package-distro-preview.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class DistroPreview(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.profile = self.base / 'profile'
        self.profile.mkdir()
        self.put('chentu/run.sh', b'#!/bin/sh\nexit 0\n')
        (self.profile / 'chentu/run.sh').chmod(0o755)
        self.put('tools/cli.tar', b'fixture archive')
        resource = {'path': 'tools/cli.tar', **module.package.identity(self.profile / 'tools/cli.tar')}
        catalog = {'schemaVersion': 1, 'distro': 'minimal', 'targets': {'k3d-arm64': {}},
                   'files': [resource], 'applications': [{'id': 'nexus', 'releases': ['nexus']}],
                   'components': {'nexus': {'files': ['tools/cli.tar'], 'requires': []}}}
        self.put('chentu/setup/install.json', json.dumps(catalog).encode())
        self.put('setup/release-manifest.json', json.dumps({'release': '0.2.1',
                 'profiles': {'k3d-arm64': {}}, 'sources': {'chentu': {'commit': 'a' * 40}}}).encode())
        provenance = {'schemaVersion': 1, 'kind': 'chentu-deployment', 'version': '0.2.1',
                      'revision': 'a' * 40, 'sourceOverlay': {},
                      'files': {'run.sh': module.package.identity(self.profile / 'chentu/run.sh')['sha256']},
                      'generatedFiles': {'setup/install.json': module.package.identity(self.profile / 'chentu/setup/install.json')['sha256']}}
        self.put('chentu/chentu-package.json', json.dumps(provenance).encode())
        self.sums()

    def put(self, name, content):
        path = self.profile / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)

    def sums(self):
        entries = [f'{module.package.identity(p)["sha256"]}  {p.relative_to(self.profile).as_posix()}'
                   for p in sorted(self.profile.rglob('*')) if p.is_file() and p.name != 'SHA256SUMS']
        self.put('SHA256SUMS', ('\n'.join(entries) + '\n').encode())

    def test_online_archive_retains_provenance_and_cannot_promote_a_channel(self):
        before = (self.profile / 'chentu/setup/install.json').read_bytes()
        out = self.base / 'out'
        with patch.object(module.package, 'verify_public_resource') as verify:
            preview = module.prepare(self.profile, out)
            self.assertEqual(verify.call_count, 1)
        self.assertEqual((self.profile / 'chentu/setup/install.json').read_bytes(), before)
        self.assertEqual(preview['acceptance'], 'not-yet-accepted')
        upload = json.loads((out / 'upload-plan.json').read_text())['objects']
        self.assertEqual(list(upload), ['apeiron/blobs/sha256/' + preview['archive']['sha256']])
        with tarfile.open(out / 'chentu-preview.tar.gz') as archive:
            self.assertNotIn('tools/cli.tar', archive.getnames())
            self.assertEqual(archive.getmember('chentu/run.sh').mode, 0o755)
            data = archive.extractfile('chentu/setup/install.json').read()
            catalog = json.loads(data)
            provenance = json.load(archive.extractfile('chentu/chentu-package.json'))
            self.assertEqual(provenance['generatedFiles']['setup/install.json'], hashlib.sha256(data).hexdigest())
            resource = catalog['files'][0]
            self.assertEqual(resource['url'], module.package.PUBLIC + 'apeiron/blobs/sha256/' + resource['sha256'])

    def test_tampered_runtime_cannot_be_packaged_even_with_new_transport_checksum(self):
        self.put('chentu/run.sh', b'changed source')
        self.sums()
        with patch.object(module.package, 'verify_public_resource'):
            with self.assertRaisesRegex(ValueError, 'source provenance'):
                module.prepare(self.profile, self.base / 'out')

    def test_missing_public_resource_blocks_preview(self):
        with patch.object(module.package, 'verify_public_resource', side_effect=ValueError('unavailable')):
            with self.assertRaisesRegex(ValueError, 'unavailable'):
                module.prepare(self.profile, self.base / 'out')
        self.assertFalse((self.base / 'out').exists())

    def test_known_source_listing_repair_preserves_source_and_resource_identities(self):
        path = self.profile / 'chentu/setup/install.json'
        catalog = json.loads(path.read_text())
        source = 'sources/deployment-source.tar.gz'
        self.put(source, b'fixed source archive')
        resource = {'path': source, **module.package.identity(self.profile / source)}
        catalog['files'].append(resource)
        catalog['components']['tools'] = {'files': [source, source], 'requires': []}
        self.put('chentu/setup/install.json', json.dumps(catalog).encode())
        provenance = json.loads((self.profile / 'chentu/chentu-package.json').read_text())
        provenance['generatedFiles']['setup/install.json'] = module.package.identity(path)['sha256']
        self.put('chentu/chentu-package.json', json.dumps(provenance).encode())
        self.sums()
        before = path.read_bytes()
        with patch.object(module.package, 'verify_public_resource'):
            preview = module.prepare(self.profile, self.base / 'out')
        self.assertEqual(path.read_bytes(), before)
        self.assertEqual(preview['catalogRepairs'], ['tools: deduplicate ' + source])
        with tarfile.open(self.base / 'out/chentu-preview.tar.gz') as archive:
            updated = json.load(archive.extractfile('chentu/setup/install.json'))
            self.assertEqual(updated['components']['tools']['files'], [source])
            actual = next(item for item in updated['files'] if item['path'] == source)
            self.assertEqual({k: actual[k] for k in resource}, resource)
            self.assertEqual(archive.extractfile('chentu/run.sh').read(), b'#!/bin/sh\nexit 0\n')


if __name__ == '__main__':
    unittest.main()
