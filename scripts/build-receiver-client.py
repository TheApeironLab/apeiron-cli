#!/usr/bin/env python3
"""Build and measure one native receiver client using the unified CLI source."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile


def run(*args):
    return subprocess.check_output([str(a) for a in args], text=True).strip()


def build(source, candidate, platform, out):
    identity = json.loads(candidate.read_text())
    version, revision = identity['version'], identity['sources']['coordinator']
    if run('git', '-C', source, 'rev-parse', 'HEAD') != revision or run('git', '-C', source, 'status', '--porcelain'):
        raise ValueError('receiver source must be clean at the candidate commit')
    # Keep the old download layout while supported Apeiron installers still
    # require it; retire this repackaging when those server versions are retired.
    # The executable itself always comes from the sole CLI release builder.
    release = out / 'release'
    subprocess.run(['bun', 'scripts/build-release.js', str(release), platform], cwd=source,
                   env={**os.environ, 'APEIRON_BUILD_VERSION': version}, check=True)
    binary = out / 'apeiron'
    with tarfile.open(release / f'apeiron-{version}-{platform}.tar.gz') as archive:
        binary.write_bytes(archive.extractfile('apeiron').read())
        readme = archive.extractfile('README.md').read()
    binary.chmod(0o755)
    if run(binary, '--version') != version:
        raise ValueError('receiver executable version mismatch')
    name = f'apeiron-v{version}-{platform}'
    stage = out / 'stage' / name
    stage.mkdir(parents=True, exist_ok=False)
    if platform.startswith('darwin-'):
        app = stage / 'Apeiron.app'
        contents = app / 'Contents'
        (contents / 'MacOS').mkdir(parents=True)
        (contents / 'Resources').mkdir()
        shutil.copy2(binary, contents / 'MacOS/apeiron')
        shutil.copy2(source / 'runtime/assets/Info.plist', contents)
        shutil.copy2(source / 'runtime/assets/apeiron.icns', contents / 'Resources')
        run('codesign', '--force', '--sign', '-', app)
        run('codesign', '--verify', '--strict', app)
    else:
        shutil.copy2(binary, stage / 'apeiron')
    # The release README carries the bundled runtime licenses; preserve it
    # through the legacy installer layout instead of replacing it with source docs.
    (stage / 'README.md').write_bytes(readme)
    shutil.copy2(source / 'runtime/NOTICE', stage / 'NOTICE')
    dest = out / 'local-computer/apeiron' / ('v' + version)
    dest.mkdir(parents=True)
    archive = dest / (name + '.tar.gz')
    with tarfile.open(archive, 'x:gz') as target:
        target.add(stage, arcname=name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (dest / (archive.name + '.sha256')).write_text(digest + '  ' + archive.name + '\n')
    (out / f'receiver-{platform}.json').write_text(json.dumps({'version': version, 'revision': revision, 'platform': platform,
        'sha256': digest, 'signing': 'ad-hoc' if platform.startswith('darwin-') else None}) + '\n')
    shutil.rmtree(out / 'stage')
    shutil.rmtree(release)
    binary.unlink()


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    for key in ('source', 'candidate', 'out'):
        p.add_argument('--' + key, type=Path, required=True)
    p.add_argument('--platform', choices=['darwin-arm64', 'darwin-x64', 'linux-x64'], required=True)
    a = p.parse_args()
    build(a.source.resolve(), a.candidate.resolve(), a.platform, a.out.resolve())
