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

    def test_gpu_images_cannot_pass_publication(self):
        with self.assertRaisesRegex(ValueError, 'GPU host images'):
            scope.validate_catalog(self.catalog(), ['gpustack'])
        catalog = self.catalog()
        catalog['components']['gpustack'].update(images=[], files=['service.yaml'])
        catalog['files'] = [f for f in catalog['files'] if not f['path'].startswith('images/')]
        scope.validate_catalog(catalog, ['gpustack'])
        catalog['files'].append({'path': 'images/vllm_vllm-openai_cuda.tar'})
        with self.assertRaisesRegex(ValueError, 'GPU host images'):
            scope.validate_catalog(catalog, ['gpustack'])

    def test_conflicting_receipts_are_rejected_even_when_components_share_an_image(self):
        image = {'file': 'core.bin', 'reference': 'example/shared:fixed', 'digest': 'sha256:' + 'a' * 64}
        components = {'first': {'images': [image]}, 'second': {'images': [dict(image)]}}
        pins = scope.catalog_image_pins(components, 'registry.example.internal:5000')
        self.assertEqual(len(pins), 1)
        components['second']['images'][0]['digest'] = 'sha256:' + 'b' * 64
        with self.assertRaisesRegex(ValueError, 'conflicting catalog image'):
            scope.catalog_image_pins(components, 'registry.example.internal:5000')

    def material(self, root, catalog):
        material = root / 'material'
        (material / 'chentu/setup').mkdir(parents=True)
        for item in catalog['files']:
            data = item['path'].encode()
            item.update(size=len(data), sha256=hashlib.sha256(data).hexdigest())
            (material / item['path']).parent.mkdir(parents=True, exist_ok=True)
            (material / item['path']).write_bytes(data)
        (material / 'chentu/setup/install.json').write_text(json.dumps(catalog))
        return material

    def test_freezing_rejects_gpu_archives_instead_of_dropping_them(self):
        freeze = load('freeze_external_gpu', 'freeze-platform.py')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'GPU host images'):
                freeze.freeze(self.material(root, self.catalog()), 'k3d-arm64', root / 'out')

    def test_seed_holds_receipts_and_values_but_no_rebuilt_material(self):
        freeze = load('freeze_receipts', 'freeze-platform.py')
        app = {'file': 'images/apeiron_app_1.tar', 'reference': 'apeiron/app:1', 'digest': 'sha256:' + 'a' * 64}
        web = {'file': 'images/web.tar', 'reference': 'example/web:1', 'digest': 'sha256:' + 'b' * 64}
        paths = ['bin/tool', app['file'], web['file'], 'charts/x.tgz', 'files/system-skills.bundle']
        catalog = {'schemaVersion': 1, 'files': [{'path': p} for p in paths], 'components': {
            'tools': {'requires': [], 'files': ['bin/tool']},
            'nexus': {'requires': [], 'files': [web['file'], 'charts/x.tgz'], 'images': [web]},
            'apeiron': {'requires': ['nexus'], 'files': [app['file'], web['file'], 'files/system-skills.bundle'],
                        'images': [app, web]}},
            'targets': {'k3d-arm64': {'base': ['tools'], 'k3sAirgap': 'bin/tool', 'clusterOidc': True, 'environment': {
                'releases': {'nexus': {'enabled': False}, 'apeiron': {'enabled': False, 'values': {
                    'image': {'app': '__REGISTRY__/apeiron/app:1'}, 'systemSkills': {'revision': 'c' * 40}, 'replicaCount': 1}}}}}}}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            freeze.freeze(self.material(root, catalog), 'k3d-arm64', root / 'out')
            seed = json.loads((root / 'out/k3d-arm64.json').read_text())
        self.assertEqual(seed['schemaVersion'], 2)
        self.assertEqual(seed['runtime'], {'base': ['tools'], 'k3sAirgap': 'bin/tool', 'target': 'k3d-arm64', 'files': ['bin/tool']})
        self.assertEqual(seed['receipts'], [{'components': ['nexus', 'apeiron'], **web},
                                            {'components': ['nexus'], 'files': ['charts/x.tgz']}])
        self.assertEqual(seed['values'], {'apeiron': {'systemSkills': {}, 'replicaCount': 1}})
        self.assertEqual(set(seed['assets']), {'bin/tool', 'images/web.tar', 'charts/x.tgz'})


if __name__ == '__main__':
    unittest.main()
