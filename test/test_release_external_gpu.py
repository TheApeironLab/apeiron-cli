import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import sys
import unittest

SCRIPTS = Path(__file__).parents[1] / 'scripts'
sys.path.insert(0, str(SCRIPTS))

def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

scope = load('external_gpu_scope', 'distribution_scope.py')

class ExternalGpuMaterial(unittest.TestCase):
    def catalog(self):
        return {'schemaVersion': 1, 'targets': {'k3d-arm64': {'environment': {'releases': {'gpustack': {'enabled': True}}}}},
                'components': {'nexus': {'requires': [], 'files': ['core.bin']},
                    'gpustack': {'requires': ['nexus'], 'files': ['images/gpustack_runner_cuda.tar', 'service.yaml'],
                        'images': [{'file': 'images/gpustack_runner_cuda.tar', 'reference': 'gpustack/runner:fixed', 'digest': 'sha256:' + 'a' * 64}],
                        'values': {'releases': {'gpustack': {'enabled': True}}}}},
                'files': [{'path': p} for p in ['core.bin', 'images/gpustack_runner_cuda.tar', 'service.yaml']]}

    def test_projection_preserves_service_configuration_and_other_components(self):
        original = self.catalog()
        expected = copy.deepcopy(original)
        projected = scope.external_service_catalog(original)
        self.assertEqual(original, expected)
        self.assertEqual(projected['components']['gpustack']['files'], ['service.yaml'])
        self.assertEqual(projected['components']['gpustack']['images'], [])
        self.assertEqual(projected['components']['gpustack']['requires'], ['nexus'])
        self.assertEqual(projected['components']['gpustack']['values'], original['components']['gpustack']['values'])
        self.assertEqual(projected['targets']['k3d-arm64']['environment'], {
            **original['targets']['k3d-arm64']['environment'], 'imagePins': {}})
        self.assertEqual(projected['components']['nexus'], original['components']['nexus'])
        self.assertEqual(scope.external_service_catalog(projected), projected)
        scope.validate_catalog(projected, ['gpustack'])

    def test_stale_or_orphaned_gpu_images_cannot_pass_publication(self):
        with self.assertRaisesRegex(ValueError, 'GPU host images'):
            scope.validate_catalog(self.catalog(), ['gpustack'])
        projected = scope.external_service_catalog(self.catalog())
        projected['files'].append({'path': 'images/vllm_vllm-openai_cuda.tar'})
        with self.assertRaisesRegex(ValueError, 'GPU host images'):
            scope.validate_catalog(projected, ['gpustack'])

    def test_shared_images_are_not_silently_removed_from_other_applications(self):
        catalog = self.catalog()
        catalog['components']['nexus']['files'].append('images/gpustack_runner_cuda.tar')
        with self.assertRaisesRegex(ValueError, 'unexpectedly shared'):
            scope.external_service_catalog(catalog)

    def test_removed_gpu_images_do_not_survive_in_derived_pins(self):
        catalog = self.catalog()
        environment = catalog['targets']['k3d-arm64']['environment']
        environment['imagePins'] = {'__REGISTRY__/gpustack/runner:fixed': '__REGISTRY__/gpustack/runner:fixed@sha256:' + 'a' * 64}
        projected = scope.external_service_catalog(catalog)
        self.assertEqual(projected['targets']['k3d-arm64']['environment']['imagePins'], {})
        scope.validate_catalog(projected, ['gpustack'])

    def test_rebuilt_image_replaces_old_digest_and_alias_but_not_deployment_values(self):
        catalog = scope.external_service_catalog(self.catalog())
        component = catalog['components']['nexus']
        component['images'] = [{'file': 'core.bin', 'reference': 'example/browser:source', 'digest': 'sha256:' + 'b' * 64}]
        environment = catalog['targets']['k3d-arm64']['environment']
        environment['imagePins'] = {'__REGISTRY__/example/browser:old': '__REGISTRY__/example/browser:old@sha256:' + 'a' * 64}
        # Keep stale real values visible to Chentu's separate image validator.
        environment['releases']['browser'] = {'values': {'image': '__REGISTRY__/example/browser:old'}}
        with self.assertRaisesRegex(ValueError, 'imagePins differ'):
            scope.validate_catalog(catalog, ['nexus'])
        refreshed = scope.refresh_catalog_image_pins(catalog)
        updated = refreshed['targets']['k3d-arm64']['environment']
        self.assertEqual(updated['imagePins'], {'__REGISTRY__/example/browser:source': '__REGISTRY__/example/browser:source@sha256:' + 'b' * 64})
        self.assertEqual(updated['releases'], environment['releases'])
        self.assertIn('__REGISTRY__/example/browser:old', environment['imagePins'])
        scope.validate_catalog(refreshed, ['nexus'])

    def test_conflicting_receipts_are_rejected_even_when_components_share_an_image(self):
        image = {'file': 'core.bin', 'reference': 'example/shared:fixed', 'digest': 'sha256:' + 'a' * 64}
        components = {'first': {'images': [image]}, 'second': {'images': [dict(image)]}}
        pins = scope.catalog_image_pins(components, 'registry.example.internal:5000')
        self.assertEqual(len(pins), 1)
        components['second']['images'][0]['digest'] = 'sha256:' + 'b' * 64
        with self.assertRaisesRegex(ValueError, 'conflicting catalog image'):
            scope.catalog_image_pins(components, 'registry.example.internal:5000')

    def test_freezing_does_not_read_or_publish_excluded_gpu_archive(self):
        # The GPU archive deliberately does not exist. Freezing must not fetch it.
        freeze = load('freeze_external_gpu', 'freeze-platform.py')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); material = root / 'material'
            (material / 'chentu/setup').mkdir(parents=True)
            catalog = self.catalog()
            for item in catalog['files']:
                data = item['path'].encode()
                item.update(size=len(data), sha256=hashlib.sha256(data).hexdigest())
                if not item['path'].startswith('images/'):
                    (material / item['path']).write_bytes(data)
            (material / 'chentu/setup/install.json').write_text(json.dumps(catalog))
            freeze.freeze(material, 'k3d-arm64', root / 'out')
            seed = json.loads((root / 'out/k3d-arm64.json').read_text())
            plan = json.loads((root / 'out/upload-plan.json').read_text())
            self.assertEqual(set(seed['assets']), {'core.bin', 'service.yaml'})
            self.assertFalse(any('gpustack_runner' in record['path'] for record in plan['objects'].values()))
