import copy
import json
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest.mock import Mock, patch
import importlib.util
from pathlib import Path
import sys
import unittest

scripts = Path(__file__).parents[1] / 'scripts'
sys.path.insert(0, str(scripts))
spec = importlib.util.spec_from_file_location('prepare_material', scripts / 'prepare-deployment-material.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class PublisherRenderInputs(unittest.TestCase):
    def test_external_gpu_endpoint_is_only_in_temporary_render_input(self):
        environment = {'gpustack': {'oidc': {'enabled': True}}, 'apeiron': {'image': '__REGISTRY__/apeiron/app:0.2.1'}}
        before = copy.deepcopy(environment)
        values = module.render_values(environment, 'arm64', ['apeiron', 'gpustack'])
        self.assertEqual(values['gpustack']['gpuHost'], '192.0.2.20')
        self.assertEqual(values['gpustack']['gpuPort'], 18080)
        self.assertEqual(values['apeiron']['image'], '${REGISTRY}/apeiron/app:0.2.1')
        self.assertEqual(environment, before)
        self.assertNotIn('gpuHost', environment['gpustack'])

    def test_base_render_does_not_add_optional_gpu_service(self):
        values = module.render_values({'apeiron': {}}, 'amd64', ['apeiron'])
        self.assertNotIn('gpustack', values)


class ApplicationImageConversion(unittest.TestCase):
    def test_source_identity_domains_use_the_verified_config_after_existing_loopback_conversion(self):
        for kind in ('config', 'oci-manifest', 'oci-index'):
            with self.subTest(kind=kind), TemporaryDirectory() as directory:
                root = Path(directory)
                layout = root / 'oci'
                source_id, config_digest, target_digest = ('sha256:' + value * 64 for value in ('a', 'b', 'c'))
                identity = {'imageIdKind': kind, 'configDigest': config_digest}
                hosted = SimpleNamespace(image_archive_identity=Mock(return_value=(identity, [])))
                native = SimpleNamespace(annotate=Mock())
                def run(argv):
                    if argv[1] == 'digest':
                        return target_digest
                    if argv[1] == 'pull':
                        blobs = layout / 'blobs/sha256'
                        blobs.mkdir(parents=True)
                        (blobs / target_digest[7:]).write_text(json.dumps({'config': {'digest': config_digest}}))
                        (blobs / config_digest[7:]).write_text(json.dumps({'config': {'Labels': {'source': 'fixed'}}}))
                    return ''
                with patch.object(module, 'run', side_effect=run) as calls:
                    digest, config, actual = module.convert_application_image(
                        root / 'image.tar', source_id, 'amd64', '127.0.0.1:5000',
                        'apeiron/backend:example', layout, hosted, native)
                self.assertEqual(digest, target_digest)
                self.assertEqual(actual, identity)
                self.assertEqual(config['config']['Labels'], {'source': 'fixed'})
                hosted.image_archive_identity.assert_called_once_with(root / 'image.tar', source_id, 'amd64')
                self.assertEqual([call.args[0][1] for call in calls.call_args_list], ['push', 'digest', 'pull'])
                native.annotate.assert_called_once_with(layout, 'apeiron/backend:example', target_digest, 'amd64')

    def test_unverified_source_or_nonloopback_transport_fails_before_crane(self):
        hosted = SimpleNamespace(image_archive_identity=Mock(side_effect=ValueError('unverified source')))
        with patch.object(module, 'run') as calls:
            for registry in ('registry.example.internal:5000', '127.0.0.1.example:5000', '127.0.0.1:5000'):
                with self.subTest(registry=registry), self.assertRaises(ValueError):
                    module.convert_application_image(Path('image.tar'), 'sha256:' + 'a' * 64,
                        'amd64', registry, 'apeiron/backend:example', Path('oci'), hosted, None)
            calls.assert_not_called()

if __name__ == '__main__':
    unittest.main()
