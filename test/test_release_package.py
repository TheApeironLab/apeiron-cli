import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('package_distribution', Path(__file__).parents[1] / 'scripts/package-distribution.py')
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class DistributionPackage(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.candidate = {'version': '0.2.1', 'sources': {'chentu': 'c' * 40, 'apeiron': 'a' * 40,
                          'coordinator': 'd' * 40, 'systemSkills': 'b' * 40}}
        self.outputs = {}
        for profile, arch in package.PROFILES.items():
            directory = self.root / profile
            self.outputs[profile] = directory
            runtime = b'#!/bin/sh\nexit 0\n'
            source = {'chentu': {'commit': 'c' * 40}, 'apeiron': {'commit': 'a' * 40, 'version': '0.2.1'},
                      'apeironCli': {'commit': 'd' * 40}, 'systemSkills': {'commit': 'b' * 40}}
            manifest = {'release': '0.2.1', 'profiles': {profile: {'architecture': arch}}, 'sources': source,
                        'artifacts': [{'kind': 'image', 'image': {'sourceReference': f'apeiron/{name}:0.2.1', 'targetDigest': 'sha256:' + 'e' * 64}} for name in ('backend', 'app', 'ops', 'scode')]}
            self.put(directory, 'chentu/driver.sh', runtime)
            self.put(directory, 'setup/release-manifest.json', json.dumps(manifest).encode())
            self.put(directory, 'resource.bin', b'platform bytes')
            catalog = {'schemaVersion': 1, 'targets': {profile: {}}, 'components': {'core': {'files': ['resource.bin']}},
                       'files': [{'path': 'resource.bin', **package.identity(directory / 'resource.bin')}]}
            self.put(directory, 'chentu/setup/install.json', json.dumps(catalog).encode())
            metadata = {'schemaVersion': 1, 'kind': 'chentu-deployment', 'version': '0.2.1', 'revision': 'c' * 40,
                        'sourceTree': 'f' * 40, 'sourceNormalization': {}, 'runtimePayload': {}, 'entrypoints': {},
                        'sourceOverlay': {}, 'files': {'driver.sh': hashlib.sha256(runtime).hexdigest()}}
            self.put(directory, 'chentu/chentu-package.json', json.dumps(metadata).encode())
            self.sums(directory)

    def put(self, directory, name, data):
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def sums(self, directory):
        rows = [package.identity(p)['sha256'] + '  ' + p.relative_to(directory).as_posix()
                for p in sorted(directory.rglob('*')) if p.is_file() and p.name != 'SHA256SUMS']
        (directory / 'SHA256SUMS').write_text('\n'.join(rows) + '\n')

    def test_reproducible_combined_source_with_public_content_addresses(self):
        first = package.combine(self.outputs, self.candidate, self.root / 'one')
        second = package.combine(self.outputs, self.candidate, self.root / 'two')
        self.assertEqual(first, second)
        self.assertEqual(set(first['apeiron']['images']['backend']), {'amd64', 'arm64'})
        with tarfile.open(self.root / 'one/chentu-0.2.1.tar.gz') as archive:
            catalog = json.load(archive.extractfile('chentu-0.2.1/setup/install.json'))
            self.assertEqual(set(catalog['catalogs']), set(package.PROFILES))
            for value in catalog['catalogs'].values():
                self.assertEqual(value['files'][0]['url'], package.PUBLIC + 'apeiron/blobs/sha256/' + value['files'][0]['sha256'])
            self.assertTrue(all(m.isfile() for m in archive.getmembers()))
        with self.assertRaises(FileExistsError):
            package.combine(self.outputs, self.candidate, self.root / 'one')

    def test_missing_profile_changed_resource_and_wrong_source_fail_before_output(self):
        output = self.root / 'result'
        with self.assertRaises(ValueError):
            package.combine({'k3d-arm64': self.outputs['k3d-arm64']}, self.candidate, output)
        wrong = copy.deepcopy(self.candidate)
        wrong['sources']['chentu'] = 'b' * 40
        with self.assertRaises(ValueError):
            package.combine(self.outputs, wrong, output)
        (self.outputs['k3d-arm64'] / 'resource.bin').write_bytes(b'changed')
        with self.assertRaises(ValueError):
            package.combine(self.outputs, self.candidate, output)
        self.assertFalse(output.exists())

    def test_wrong_builder_skills_and_invalid_nightly_are_rejected(self):
        for source in ('coordinator', 'systemSkills'):
            candidate = copy.deepcopy(self.candidate)
            candidate['sources'][source] = '0' * 40
            with self.assertRaises(ValueError):
                package.combine(self.outputs, candidate, self.root / 'bad')
        for version in ('0.2.1-nightly.20260230.1', '0.2.1-rc.5', '0.2.1-nightly.20261023.0'):
            with self.assertRaises(ValueError):
                package.combine(self.outputs, {**self.candidate, 'version': version}, self.root / 'bad')
        self.assertFalse((self.root / 'bad').exists())

    def test_source_metadata_conflict_fails_even_when_runtime_bytes_match(self):
        directory = self.outputs['k3s-amd64']
        path = directory / 'chentu/chentu-package.json'
        value = json.loads(path.read_text())
        value['sourceTree'] = '0' * 40
        path.write_text(json.dumps(value))
        self.sums(directory)
        with self.assertRaisesRegex(ValueError, 'provenance differs'):
            package.combine(self.outputs, self.candidate, self.root / 'bad')
        self.assertFalse((self.root / 'bad').exists())

    def test_path_escape_and_symlink_are_rejected(self):
        for value in ('../file', '/file', 'a//b', '.git/config', 'a/./b', 'a\\b'):
            with self.assertRaises(ValueError):
                package.safe_file(self.root, value)
        (self.root / 'link').symlink_to(self.root / 'k3d-arm64', target_is_directory=True)
        with self.assertRaises(ValueError):
            package.safe_file(self.root, 'link/resource.bin')


if __name__ == '__main__':
    unittest.main()
