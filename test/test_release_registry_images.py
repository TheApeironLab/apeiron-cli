import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('registry_export', Path(__file__).parents[1] / 'scripts/export-platform-image.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class PlatformBuildScopeTests(unittest.TestCase):
    def test_browser_uses_third_party_material_without_a_private_build(self):
        """Regression for TheApeironLab/apeiron-cli#95."""
        lock = json.loads((Path(__file__).parents[1] / 'release/platform-images.json').read_text())
        for group in ('images', 'registryImages', 'sourceBuilds'):
            self.assertNotIn('chrome-headless', lock.get(group, {}))
        self.assertTrue({'zygos-api', 'zygos-web', 'zygos-worker', 'zygos-evalscope'} <= lock['images'].keys())
        self.assertIn('sftpgo', lock['sourceBuilds'])


class RegistryClosureTests(unittest.TestCase):
    def fixture(self, directory, *, omit_layer=False, corrupt_layer=False, arches=('amd64', 'arm64')):
        blobs = {}

        def blob(value):
            data = json.dumps(value).encode() if isinstance(value, dict) else value
            checksum = hashlib.sha256(data).hexdigest()
            blobs['blobs/sha256/' + checksum] = data
            return {'digest': 'sha256:' + checksum, 'size': len(data)}

        layer = blob(b'fixture image layer')
        manifests = []
        for arch in arches:
            config = blob({'os': 'linux', 'architecture': arch})
            manifests.append(blob({'schemaVersion': 2, 'config': config, 'layers': [layer]}))
        root = blob({'schemaVersion': 2, 'manifests': manifests})
        if omit_layer:
            del blobs['blobs/sha256/' + layer['digest'][7:]]
        if corrupt_layer:
            blobs['blobs/sha256/' + layer['digest'][7:]] = b'corrupt image layer'
        blobs['index.json'] = json.dumps({'schemaVersion': 2, 'manifests': [root]}).encode()
        blobs['oci-layout'] = b'{"imageLayoutVersion":"1.0.0"}'
        path = Path(directory) / 'fixture.tar'
        with tarfile.open(path, 'w') as archive:
            for name, data in blobs.items():
                info = tarfile.TarInfo(name)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        return path, root['digest']

    def test_verifies_both_architectures_and_pinned_root(self):
        with tempfile.TemporaryDirectory() as directory:
            path, root = self.fixture(directory)
            self.assertEqual(module.verify_archive(path, root), ['linux/amd64', 'linux/arm64'])
            with self.assertRaisesRegex(ValueError, 'pinned registry digest'):
                module.verify_archive(path, 'sha256:' + '0' * 64)

    def test_rejects_missing_or_corrupt_layers(self):
        for option in ('omit_layer', 'corrupt_layer'):
            with self.subTest(option=option), tempfile.TemporaryDirectory() as directory:
                path, root = self.fixture(directory, **{option: True})
                with self.assertRaisesRegex(ValueError, 'OCI (graph member|blob checksum)'):
                    module.verify_archive(path, root)

    def test_rejects_single_architecture_export(self):
        with tempfile.TemporaryDirectory() as directory:
            path, root = self.fixture(directory, arches=('amd64',))
            with self.assertRaisesRegex(ValueError, 'both linux/amd64 and linux/arm64'):
                module.verify_archive(path, root)

    def test_tag_is_metadata_and_transport_uses_only_the_fixed_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, root = self.fixture(directory)
            with patch.object(module.subprocess, 'run') as run:
                module.export_archive('ghcr.io/example/image:display-tag@' + root, archive)
                command = run.call_args.args[0]
                self.assertIn('docker://ghcr.io/example/image@' + root, command)
                self.assertTrue(all('display-tag' not in arg for arg in command))
            with patch.object(module.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'fixed SHA-256 digest'):
                    module.export_archive('ghcr.io/example/image:latest', archive)
                run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
