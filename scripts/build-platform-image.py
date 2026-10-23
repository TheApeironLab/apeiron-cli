#!/usr/bin/env python3
"""Build a reviewed optional application's image and retain measured provenance."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def run(command):
    return subprocess.check_output([str(x) for x in command], text=True).strip()


def digest(path):
    hasher = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            hasher.update(chunk)
    return hasher.hexdigest()


p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--source', type=Path, required=True)
p.add_argument('--component', required=True)
p.add_argument('--arch', choices=['arm64', 'amd64'], required=True)
p.add_argument('--out', type=Path, required=True)
a = p.parse_args()
lock = json.loads((Path(__file__).parents[1] / 'release/platform-images.json').read_text())
item = lock['images'][a.component]
revision = lock['sources'][item['source']]
if run(['git', '-C', a.source, 'rev-parse', 'HEAD']) != revision or run(['git', '-C', a.source, 'status', '--porcelain']):
    raise ValueError('optional image requires the exact clean reviewed source')
a.out.mkdir(parents=True, exist_ok=False)
reference = f'theapeironlab/{a.component}:source-{revision[:12]}'
recipe = a.source / item['recipe']
subprocess.run(['docker', 'build', '--platform', 'linux/' + a.arch,
    '--label', 'org.opencontainers.image.revision=' + revision,
    '--label', 'org.opencontainers.image.source=https://github.com/TheApeironLab/' + item['source'],
    '-f', str(recipe), '-t', reference, str(a.source / item['context'])], check=True)
image = json.loads(run(['docker', 'image', 'inspect', reference]))[0]
if image['Architecture'] != a.arch or image['Config']['Labels']['org.opencontainers.image.revision'] != revision:
    raise ValueError('built image does not match its source or target')
archive = a.out / (a.component + '-' + a.arch + '.tar')
with archive.open('xb') as stream:
    subprocess.run(['docker', 'save', reference], stdout=stream, check=True)
proof = {'source': {'repository': 'https://github.com/TheApeironLab/' + item['source'], 'commit': revision, 'clean': True},
         'recipe': {'file': item['recipe'], 'sha256': digest(recipe)}, 'platform': 'linux/' + a.arch,
         'reference': reference, 'imageId': image['Id'],
         'archive': {'file': archive.name, 'sha256': digest(archive), 'size': archive.stat().st_size},
         'acceptanceComplete': False}
(a.out / 'build-provenance.json').write_text(json.dumps(proof, indent=2) + '\n')
