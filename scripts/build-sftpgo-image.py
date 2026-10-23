#!/usr/bin/env python3
"""Rebuild SFTPGo's reviewed PKCE source without a pre-existing private image."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


def run(command, **kwargs):
    return subprocess.check_output([str(x) for x in command], text=True, **kwargs).strip()


def digest(path):
    hasher = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            hasher.update(chunk)
    return hasher.hexdigest()


def build(source, arch, out):
    lock = json.loads((Path(__file__).parents[1] / 'release/platform-images.json').read_text())['sourceBuilds']['sftpgo']
    revision = lock['revision']
    if (run(['git', '-C', source, 'rev-parse', 'HEAD']) != revision
            or run(['git', '-C', source, 'status', '--porcelain'])):
        raise ValueError('SFTPGo requires the exact clean reviewed source')
    recipe = source / lock['recipe']
    if digest(recipe) != lock['recipeSha256']:
        raise ValueError('SFTPGo Dockerfile differs from the reviewed recipe')
    go_version = run(['go', 'version'])
    if go_version.split()[2] != 'go' + lock['goVersion']:
        raise ValueError('SFTPGo requires the locked Go compiler')
    out.mkdir(parents=True, exist_ok=False)
    # Keep generated binary and CA bundle out of the source checkout. The original
    # Dockerfile consumes these explicitly, as its upstream workflow specifies.
    context = out / 'context'
    shutil.copytree(source, context, ignore=shutil.ignore_patterns('.git'))
    binary = context / 'sftpgo'
    environment = {**os.environ, 'CGO_ENABLED': '0', 'GOOS': 'linux', 'GOARCH': arch, 'GOTOOLCHAIN': 'local'}
    subprocess.run(['go', 'build', '-mod=readonly', '-tags', lock['buildTags'], '-trimpath',
                    '-ldflags', '-s -w', '-o', str(binary), '.'], cwd=source, env=environment, check=True)
    if run(['git', '-C', source, 'status', '--porcelain']):
        raise ValueError('SFTPGo build modified the reviewed source')
    ca = context / 'ca-certificates.crt'
    shutil.copyfile('/etc/ssl/certs/ca-certificates.crt', ca)
    reference = 'theapeironlab/sftpgo:source-' + revision[:12]
    subprocess.run(['docker', 'build', '--platform', 'linux/' + arch,
        '--label', 'org.opencontainers.image.revision=' + revision,
        '--label', 'org.opencontainers.image.source=https://github.com/' + lock['repository'],
        '-f', str(context / lock['recipe']), '-t', reference, str(context)], check=True)
    image = json.loads(run(['docker', 'image', 'inspect', reference]))[0]
    if (image['Architecture'] != arch or image['Os'] != 'linux'
            or image['Config']['Labels']['org.opencontainers.image.revision'] != revision):
        raise ValueError('SFTPGo image identity or architecture differs')
    # Native runners exercise the actual cross-built binary, with no network.
    version = run(['docker', 'run', '--rm', '--network', 'none', '--read-only',
                   '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', reference, '--version'])
    if 'SFTPGo 2.7.6' not in version:
        raise ValueError('SFTPGo executable has an unexpected version')
    archive = out / ('sftpgo-' + arch + '.tar')
    with archive.open('xb') as stream:
        subprocess.run(['docker', 'save', reference], stdout=stream, check=True)
    proof = {'source': {'repository': 'https://github.com/' + lock['repository'], 'commit': revision, 'clean': True},
        'recipe': {'file': lock['recipe'], 'sha256': digest(recipe)}, 'platform': 'linux/' + arch,
        'reference': reference, 'imageId': image['Id'],
        'build': {'go': go_version, 'tags': lock['buildTags'], 'binarySha256': digest(binary), 'caSha256': digest(ca)},
        'runtimeVersion': version,
        'archive': {'file': archive.name, 'sha256': digest(archive), 'size': archive.stat().st_size},
        'acceptanceComplete': False}
    (out / 'build-provenance.json').write_text(json.dumps(proof, indent=2) + '\n')
    shutil.rmtree(context)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--arch', choices=['arm64', 'amd64'], required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    build(args.source.resolve(), args.arch, args.out.resolve())
