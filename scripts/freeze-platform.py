#!/usr/bin/env python3
"""Freeze verified reusable platform material; application builds remain separate."""
import argparse
import copy
import importlib.util
import json
from pathlib import Path
from distribution_scope import external_service_catalog, platform_build_provenance

spec = importlib.util.spec_from_file_location('package_distribution', Path(__file__).with_name('package-distribution.py'))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


def freeze(material, profile, out, toolbox_lineage=None):
    catalog = json.loads((material / 'chentu/setup/install.json').read_text())
    if catalog['schemaVersion'] != 1 or set(catalog['targets']) != {profile}:
        raise ValueError('one explicit material profile is required')
    catalog = external_service_catalog(catalog)
    catalog = platform_build_provenance(catalog, package.PROFILES[profile])
    dynamic = set()
    for component in catalog['components'].values():
        for image in component.get('images', []):
            if image['reference'].startswith('apeiron/'):
                dynamic.add(image['file'])
    dynamic.update(item['path'] for item in catalog['files']
                   if item['path'].startswith(('local-computer/apeiron/', 'sandbox-apt/', 'sources/', 'setup/'))
                   or item['path'] == 'files/system-skills.bundle'
                   or (item['path'].startswith('cli/') and Path(item['path']).name.startswith('chentu-')))
    assets, objects = {}, {}
    for item in catalog['files']:
        if item['path'] in dynamic:
            continue
        path = package.safe_file(material, item['path'])
        got = package.identity(path)
        if got != {k: item[k] for k in ('sha256', 'size')}:
            raise ValueError('seed material checksum mismatch: ' + item['path'])
        key = 'apeiron/blobs/sha256/' + got['sha256']
        assets[item['path']] = {**got, 'url': package.PUBLIC + key,
                                'mode': 0o755 if path.stat().st_mode & 0o111 else 0o644}
        objects.setdefault(key, {'path': str(path.resolve()), **got})
    seed = {'schemaVersion': 1, 'kind': 'apeiron-platform-material', 'profile': profile,
            'catalog': copy.deepcopy(catalog), 'assets': assets,
            'rebuiltResources': sorted(dynamic)}
    if toolbox_lineage:
        seed['toolboxLineage'] = json.loads(toolbox_lineage.read_text())
    # The seed only holds checksummed publisher material, never a test profile,
    # kubeconfig, model API key, SSH identity, deployment state or GPG secret.
    out.mkdir(parents=True, exist_ok=False)
    seed_path = out / (profile + '.json')
    seed_path.write_text(json.dumps(seed, indent=2) + '\n')
    got = package.identity(seed_path)
    key = 'apeiron/blobs/sha256/' + got['sha256']
    objects[key] = {'path': str(seed_path.resolve()), **got}
    (out / 'upload-plan.json').write_text(json.dumps({'objects': objects}, indent=2) + '\n')
    return {**got, 'url': package.PUBLIC + key}


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--material', type=Path, required=True)
    p.add_argument('--profile', choices=package.PROFILES, required=True)
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--toolbox-lineage', type=Path)
    a = p.parse_args()
    print(json.dumps(freeze(a.material, a.profile, a.out, a.toolbox_lineage)))
