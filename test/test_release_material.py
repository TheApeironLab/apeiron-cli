import copy
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


if __name__ == '__main__':
    unittest.main()
