#!/usr/bin/env python3
"""Export a pinned registry image and verify its complete multi-platform OCI graph."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile


def digest(stream):
    hasher = hashlib.sha256()
    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
        hasher.update(chunk)
    return hasher.hexdigest()


def verify_archive(path, expected):
    with tarfile.open(path) as archive:
        members = {}
        for member in archive:
            name = member.name.removeprefix('./').rstrip('/')
            if member.isdir() and name in ('', '.', 'blobs', 'blobs/sha256'):
                continue
            if not member.isfile() or name in members or not (
                    name in ('index.json', 'oci-layout') or re.fullmatch(r'blobs/sha256/[0-9a-f]{64}', name)):
                raise ValueError('invalid or duplicate OCI archive member: ' + name)
            members[name] = member
            if name.startswith('blobs/') and digest(archive.extractfile(member)) != name.split('/')[-1]:
                raise ValueError('OCI blob checksum mismatch: ' + name)

        def read(name):
            if name not in members or members[name].size > 16 * 1024 * 1024:
                raise ValueError('missing or oversized OCI metadata: ' + name)
            return json.load(archive.extractfile(members[name]))

        if read('oci-layout') != {'imageLayoutVersion': '1.0.0'}:
            raise ValueError('unsupported OCI layout')
        roots = read('index.json')['manifests']
        if len(roots) != 1 or roots[0]['digest'] != expected:
            raise ValueError('OCI root differs from the pinned registry digest')
        platforms, visited = set(), set()

        def member_for(descriptor):
            value = descriptor['digest']
            if not re.fullmatch(r'sha256:[0-9a-f]{64}', value):
                raise ValueError('unsupported OCI digest')
            name = 'blobs/sha256/' + value[7:]
            if name not in members or members[name].size != descriptor['size']:
                raise ValueError('missing or truncated OCI graph member: ' + name)
            return name

        def walk(descriptor):
            name = member_for(descriptor)
            if name in visited:
                return
            visited.add(name)
            item = read(name)
            if 'manifests' in item:
                for child in item['manifests']:
                    walk(child)
            else:
                config = read(member_for(item['config']))
                platforms.add(config.get('os', '') + '/' + config.get('architecture', ''))
                for layer in item['layers']:
                    member_for(layer)

        walk(roots[0])
        if not {'linux/amd64', 'linux/arm64'} <= platforms:
            raise ValueError('pinned image must contain both linux/amd64 and linux/arm64')
        return sorted(platforms)


def export_archive(reference, archive):
    tagged, separator, expected = reference.rpartition('@')
    if not separator or not re.fullmatch(r'sha256:[0-9a-f]{64}', expected):
        raise ValueError('registry exports require a fixed SHA-256 digest')
    # Skopeo rejects tag@digest, even though it is a valid display reference.
    # Keep the tag in provenance, but address the transport exclusively by digest.
    parent, slash, name = tagged.rpartition('/')
    repository = parent + slash + name.split(':', 1)[0]
    subprocess.run(['skopeo', 'copy', '--all', '--preserve-digests',
                    'docker://' + repository + '@' + expected,
                    'oci-archive:' + str(archive)], check=True)
    return verify_archive(archive, expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--component', required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    lock = json.loads((Path(__file__).parents[1] / 'release/platform-images.json').read_text())
    reference = lock['registryImages'][args.component]
    expected = reference.rsplit('@', 1)[-1]
    if not re.fullmatch(r'ghcr.io/theapeironlab/[a-z0-9-]+:[a-zA-Z0-9_.-]+@sha256:[0-9a-f]{64}', reference):
        raise ValueError('private platform images require a pinned first-party GHCR reference')
    args.out.mkdir(parents=True, exist_ok=False)
    archive = args.out / (args.component + '.oci.tar')
    platforms = export_archive(reference, archive)
    with archive.open('rb') as stream:
        checksum = digest(stream)
    (args.out / 'export-provenance.json').write_text(json.dumps({
        'reference': reference, 'digest': expected, 'platforms': platforms,
        'archive': {'file': archive.name, 'sha256': checksum, 'size': archive.stat().st_size},
        'acceptanceComplete': False,
    }, indent=2) + '\n')


if __name__ == '__main__':
    main()
