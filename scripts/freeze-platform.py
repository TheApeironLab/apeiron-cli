#!/usr/bin/env python3
"""Freeze verified reusable platform material as receipts; application builds remain separate."""
import argparse
import importlib.util
import json
from pathlib import Path
from distribution_scope import platform_build_provenance, validate_catalog

spec = importlib.util.spec_from_file_location('package_distribution', Path(__file__).with_name('package-distribution.py'))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)

REBUILT = ('local-computer/apeiron/', 'sandbox-apt/', 'sources/', 'setup/', 'files/system-skills.bundle')
GENERATED = ('environment', 'deploymentTopology', 'publicPorts', 'clusterOidc', 'hostPlatform')


def freeze(material, profile, out, toolbox_lineage=None):
    catalog = json.loads((material / 'chentu/setup/install.json').read_text())
    if catalog['schemaVersion'] != 1 or set(catalog['targets']) != {profile}:
        raise ValueError('one explicit material profile is required')
    validate_catalog(catalog, list(catalog['components']))  # scope is checked on the generated catalog
    catalog = platform_build_provenance(catalog, package.PROFILES[profile])
    images = {image['file']: image for component in catalog['components'].values() for image in component.get('images', [])}
    dynamic = {path for path, image in images.items() if image['reference'].startswith('apeiron/')}
    dynamic.update(item['path'] for item in catalog['files'] if item['path'].startswith(REBUILT)
                   or (item['path'].startswith('cli/') and Path(item['path']).name.startswith('chentu-')))
    owners = {}
    for name, component in catalog['components'].items():
        for path in component['files']:
            if path not in dynamic:
                owners.setdefault(path, []).append(name)
    # Release preparation regenerates install.json from these receipts plus the new builds.
    tools, receipts, grouped = [], [], {}
    for path, names in sorted(owners.items()):
        if 'tools' in names:
            tools.append(path)
            names.remove('tools')
        if path in images:
            receipts.append({'components': names, **images[path]})
        elif names:
            grouped.setdefault(tuple(names), []).append(path)
    receipts += [{'components': list(names), 'files': paths} for names, paths in grouped.items()]
    target = catalog['targets'][profile]
    values = {name: release['values'] for name, release in target['environment']['releases'].items() if 'values' in release}
    values.get('apeiron', {}).pop('image', None)
    values.get('apeiron', {}).get('systemSkills', {}).pop('revision', None)
    runtime = {**{k: v for k, v in target.items() if k not in GENERATED}, 'target': profile, 'files': tools}
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
    seed = {'schemaVersion': 2, 'kind': 'apeiron-platform-material', 'profile': profile,
            'runtime': runtime, 'receipts': receipts, 'values': values, 'assets': assets}
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
