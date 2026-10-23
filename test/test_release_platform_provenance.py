import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).parents[1] / 'scripts'))
from distribution_scope import platform_build_provenance


class PlatformProvenance(unittest.TestCase):
    def catalog(self):
        source = {'repository': 'https://example.invalid/source', 'commit': 'a' * 40, 'clean': True}
        builds = [{'source': copy.deepcopy(source), 'platform': 'linux/' + arch,
                   'recipe': {'file': 'Dockerfile', 'sha256': 'b' * 64},
                   'digest': 'sha256:' + digest * 64}
                  for arch, digest in (('amd64', 'c'), ('arm64', 'd'))]
        return {'components': {'browser': {'images': [{
            'file': 'images/browser.tar', 'reference': 'example/browser:fixed',
            'digest': 'sha256:' + 'e' * 64,
            'buildProvenance': {'source': source, 'digest': 'sha256:' + 'e' * 64,
                                'platformBuilds': builds, 'acceptanceComplete': False}}]}}}

    def test_selected_recipe_is_explicit_without_losing_index_or_other_platform_evidence(self):
        original = self.catalog()
        before = copy.deepcopy(original)
        for architecture in ('amd64', 'arm64'):
            projected = platform_build_provenance(original, architecture)
            image = projected['components']['browser']['images'][0]
            proof = image['buildProvenance']
            previous = original['components']['browser']['images'][0]
            self.assertEqual(proof['platform'], 'linux/' + architecture)
            self.assertEqual(proof['recipe'], {'file': 'Dockerfile', 'sha256': 'b' * 64})
            self.assertEqual(proof['platformBuilds'], previous['buildProvenance']['platformBuilds'])
            self.assertEqual(proof['digest'], previous['buildProvenance']['digest'])
            self.assertEqual(image['digest'], previous['digest'])
            self.assertEqual(image['file'], previous['file'])
            self.assertFalse(proof['acceptanceComplete'])
            self.assertEqual(platform_build_provenance(projected, architecture), projected)
        self.assertEqual(original, before)

    def test_missing_or_duplicate_platform_build_is_rejected(self):
        for builds in ([], [1, 1]):
            catalog = self.catalog()
            proof = catalog['components']['browser']['images'][0]['buildProvenance']
            proof['platformBuilds'] = [copy.deepcopy(proof['platformBuilds'][i]) for i in builds]
            with self.assertRaisesRegex(ValueError, 'requires one build'):
                platform_build_provenance(catalog, 'arm64')

    def test_conflicting_source_or_existing_recipe_cannot_be_relabelled(self):
        for field in ('source', 'recipe', 'platform'):
            catalog = self.catalog()
            proof = catalog['components']['browser']['images'][0]['buildProvenance']
            if field == 'source':
                proof['platformBuilds'][1]['source']['commit'] = 'f' * 40
            elif field == 'recipe':
                proof['recipe'] = {'file': 'different.Dockerfile', 'sha256': 'f' * 64}
            else:
                proof['platform'] = 'linux/amd64'
            with self.assertRaises(ValueError):
                platform_build_provenance(catalog, 'arm64')
