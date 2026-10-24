#!/usr/bin/env python3
"""Compose a native K3s platform seed: the architecture's frozen K3d seed plus native host material.

Images, Charts and their receipts are the same bytes as the K3d seed of that architecture;
native host resources come from Chentu `bundle/build.sh`. Template only supplies the
native base components and storage values of an existing native seed.
"""
import argparse
import importlib.util
import json
import re
from pathlib import Path

spec = importlib.util.spec_from_file_location('package_distribution', Path(__file__).with_name('package-distribution.py'))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)

UBUNTU = {'amd64': 'jammy', 'arm64': 'noble'}
# Rebuilt per release, duplicates, or build bookkeeping: never part of the seed.
SKIP = re.compile(r'^(SHA256SUMS|k3s/selected\.sha256|k3s/k3s-arm64|cli/chentu-[^/]+\.whl)$')


def compose(host, k3d, template, profile, out):
    arch = profile.removeprefix('k3s-')
    if not profile.startswith('k3s-') or arch not in UBUNTU:
        raise ValueError('a native profile is k3s-amd64 or k3s-arm64')
    if k3d['profile'] != 'k3d-' + arch or template['runtime'].get('operatorTools') is not True:
        raise ValueError('compose needs the same-architecture K3d seed and a native template seed')
    bundle = (host / 'bundle.yaml').read_text()
    if f'arch: {arch}\n' not in bundle or f'ubuntu: {UBUNTU[arch]}\n' not in bundle:
        raise ValueError('host material is not built for ' + profile)
    k3s = {'k3s/k3s', 'k3s/install.sh', 'k3s/audit-policy.yaml',
           f'k3s/k3s-airgap-images-{arch}.tar.zst', f'k3s/sha256sum-{arch}.txt'}
    assets = {path: record for path, record in k3d['assets'].items() if not path.startswith('tools/')}
    objects, files = {}, []
    for path in sorted(p for p in host.rglob('*') if p.is_file()):
        relative = path.relative_to(host).as_posix()
        if SKIP.match(relative) or (relative.startswith('k3s/') and relative not in k3s):
            continue
        got = package.identity(path)
        if relative.startswith('debs/'):
            relative = f"debs/sha256-{got['sha256']}.deb"
        key = 'apeiron/blobs/sha256/' + got['sha256']
        record = {**got, 'url': package.PUBLIC + key, 'mode': 0o755 if path.stat().st_mode & 0o111 else 0o644}
        if relative in assets and {k: assets[relative][k] for k in ('sha256', 'size')} != got:
            raise ValueError('host material differs from the K3d seed: ' + relative)
        assets[relative] = record
        objects.setdefault(key, {'path': str(path.resolve()), **got})
        files.append(relative)
    if not k3s <= set(files) or not any(f.startswith('debs/') for f in files):
        raise ValueError('host material lacks K3s or host Debs')
    runtime = {'base': template['runtime']['base'], 'operatorTools': True,
               'k3sAirgap': f'k3s/k3s-airgap-images-{arch}.tar.zst', 'target': profile, 'files': files}
    # Image digests in values follow the architecture; native storage values follow the topology.
    values = {**k3d['values'], 'seaweedfs': template['values']['seaweedfs']}
    seed = {'schemaVersion': 2, 'kind': 'apeiron-platform-material', 'profile': profile,
            'runtime': runtime, 'receipts': k3d['receipts'], 'values': values, 'assets': assets}
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
    p.add_argument('--host', type=Path, required=True)
    p.add_argument('--k3d-seed', type=Path, required=True)
    p.add_argument('--template-seed', type=Path, required=True)
    p.add_argument('--profile', required=True)
    p.add_argument('--out', type=Path, required=True)
    a = p.parse_args()
    print(json.dumps(compose(a.host.resolve(), json.loads(a.k3d_seed.read_text()),
                             json.loads(a.template_seed.read_text()), a.profile, a.out.resolve())))
