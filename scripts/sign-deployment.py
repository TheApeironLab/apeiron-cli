#!/usr/bin/env python3
"""Sign the immutable deployment archive with the reviewed release identity."""
import argparse
from datetime import datetime
import importlib.util
import json
from pathlib import Path
import re
import subprocess

spec = importlib.util.spec_from_file_location('release_files', Path(__file__).with_name('package-distribution.py'))
files = importlib.util.module_from_spec(spec)
spec.loader.exec_module(files)


def sign(directory, home, fingerprint, candidate):
    if not re.fullmatch('[A-F0-9]{40}', fingerprint):
        raise ValueError('Signing requires the reviewed full GPG fingerprint')
    plan = json.loads((directory / 'upload-plan.json').read_text())
    version = plan['version']
    if candidate['version'] != version:
        raise ValueError('Signing identity differs from the prepared candidate')
    timestamp = datetime.fromisoformat(candidate['createdAt'].replace('Z', '+00:00'))
    if timestamp.utcoffset() is None:
        raise ValueError('Candidate signing time must include its timezone')
    archive = directory / f'chentu-{version}.tar.gz'
    key = f'chentu/releases/{version}/{archive.name}'
    if files.identity(archive) != {k: plan['objects'][key][k] for k in ('sha256', 'size')}:
        raise ValueError('Archive changed before signing')
    signature = Path(str(archive) + '.asc')
    subprocess.run(['gpg', '--homedir', str(home), '--batch', '--armor', '--local-user', fingerprint + '!',
                    '--faked-system-time', str(int(timestamp.timestamp())) + '!',
                    '--output', str(signature), '--detach-sign', str(archive)], check=True)
    result = subprocess.check_output(['gpg', '--homedir', str(home), '--batch', '--status-fd', '1',
                                     '--verify', str(signature), str(archive)], text=True)
    if not any(line.startswith('[GNUPG:] VALIDSIG ' + fingerprint + ' ') for line in result.splitlines()):
        raise ValueError('Detached signature uses an unexpected key')
    plan['objects'][key + '.asc'] = {'path': str(signature.resolve()), **files.identity(signature)}
    (directory / 'upload-plan.json').write_text(json.dumps(plan, indent=2) + '\n')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--directory', type=Path, required=True)
    p.add_argument('--gnupghome', type=Path, required=True)
    p.add_argument('--fingerprint', required=True)
    p.add_argument('--candidate', type=Path, required=True)
    a = p.parse_args()
    sign(a.directory, a.gnupghome, a.fingerprint, json.loads(a.candidate.read_text()))
