#!/usr/bin/env python3
"""Prepare immutable resource uploads and a small, independently verifiable profile handoff."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil

spec = importlib.util.spec_from_file_location('profile_package', Path(__file__).with_name('package-distribution.py'))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


def prepare(directory, output, plan_path):
    if (directory == output or directory in output.parents
            or directory == plan_path or directory in plan_path.parents
            or output == plan_path or output in plan_path.parents):
        raise ValueError('handoff and upload plan must stay outside their input and each other')
    if (directory / 'profile-handoff.json').exists():
        raise ValueError('handoff preparation requires complete local material')
    _, catalog, _ = package.read_profile(directory)
    records, objects = {}, {}
    for item in package.resource_files(catalog):
        path = package.safe_file(directory, item['path'])
        got = package.identity(path)
        if got != {k: item[k] for k in ('sha256', 'size')}:
            raise ValueError('profile resource differs from catalog')
        key = 'apeiron/blobs/sha256/' + got['sha256']
        objects.setdefault(key, {'path': str(path.resolve()), **got})
        # Source provenance and the release manifest are read locally by the
        # coordinator. Large installer resources are checked over HTTPS there.
        if not item['path'].startswith('chentu/') and item['path'] != 'setup/release-manifest.json':
            records[item['path']] = {**got, 'url': package.PUBLIC + key}
    output.mkdir(parents=True, exist_ok=False)
    executables = []
    for path in directory.rglob('*'):
        if path.is_symlink():
            raise ValueError('profile handoff rejects symlinks')
        if not path.is_file():
            continue
        name = path.relative_to(directory).as_posix()
        if name in records:
            continue
        if path.stat().st_mode & 0o111:
            executables.append(name)
        destination = package.safe_file(output, name)
        destination.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(path, destination)
        except OSError:
            shutil.copy2(path, destination)
    (output / 'profile-handoff.json').write_text(json.dumps({
        'schemaVersion': 1, 'kind': 'apeiron-published-profile', 'resources': records,
        # CI artifacts do not keep file modes; the coordinator restores them from here.
        'executables': sorted(executables),
    }, indent=2) + '\n')
    # This plan must finish publication before the handoff becomes a CI artifact.
    plan_path.write_text(json.dumps({'objects': objects}, indent=2) + '\n')
    return {'remoteResources': len(records), 'uploadObjects': len(objects)}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('profile', 'out', 'upload-plan'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(prepare(args.profile.resolve(), args.out.resolve(), args.upload_plan.resolve())))
