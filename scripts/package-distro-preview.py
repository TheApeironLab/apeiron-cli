#!/usr/bin/env python3
"""Package one verified online profile for development, without publishing a release."""
import argparse
import copy
import gzip
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile

spec = importlib.util.spec_from_file_location('distribution_package', Path(__file__).with_name('package-distribution.py'))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


def prepare(directory, output):
    if directory == output or directory in output.parents or output in directory.parents:
        raise ValueError('preview output must be separate from the profile')
    manifest, catalog, provenance = package.read_profile(directory)
    catalog, provenance = copy.deepcopy(catalog), copy.deepcopy(provenance)
    package.scope.validate_catalog(catalog)
    if (provenance.get('kind') != 'chentu-deployment' or provenance.get('schemaVersion') != 1
            or 'setup/install.json' not in provenance['generatedFiles']
            or 'setup/install.json' in provenance['files']
            or len(catalog['targets']) != 1 or set(catalog['targets']) != set(manifest['profiles'])
            or provenance['version'] != manifest['release']
            or provenance['revision'] != manifest['sources']['chentu']['commit']
            or provenance['sourceOverlay'] != {}):
        raise ValueError('preview requires one consistent source profile')
    # Source and generated-file checks remain mandatory for a development preview.
    root = directory / 'chentu'
    for name, digest in provenance['generatedFiles'].items():
        if package.identity(package.safe_file(root, name))['sha256'] != digest:
            raise ValueError('generated profile file differs from provenance')
    # Repair only the known generated-catalog defect in older development
    # profiles. Runtime bytes and every resource identity remain unchanged.
    repairs = []
    for name, component in catalog['components'].items():
        paths = component['files']
        duplicates = {path for path in paths if paths.count(path) > 1}
        if duplicates:
            if name != 'tools' or duplicates != {'sources/deployment-source.tar.gz'}:
                raise ValueError('duplicate component resource in preview input')
            component['files'] = list(dict.fromkeys(paths))
            repairs.append('tools: deduplicate sources/deployment-source.tar.gz')
    for entry in catalog['files']:
        entry['url'] = package.PUBLIC + 'apeiron/blobs/sha256/' + entry['sha256']
        package.verify_public_resource({k: entry[k] for k in ('sha256', 'size', 'url')})
    files = {}
    executable = package.executables(directory)
    for name, digest in provenance['files'].items():
        path = package.safe_file(root, name)
        if package.identity(path)['sha256'] != digest:
            raise ValueError('runtime differs from source provenance')
        mode = (path.stat().st_mode & 0o111) if executable is None else f'chentu/{name}' in executable
        files[name] = (path.read_bytes(), 0o755 if mode else 0o644)
    catalog_bytes = (json.dumps(catalog, indent=2) + '\n').encode()
    provenance['generatedFiles']['setup/install.json'] = hashlib.sha256(catalog_bytes).hexdigest()
    for name in provenance['generatedFiles']:
        data = catalog_bytes if name == 'setup/install.json' else package.safe_file(root, name).read_bytes()
        files[name] = (data, 0o644)
    files['chentu-package.json'] = ((json.dumps(provenance, indent=2) + '\n').encode(), 0o644)
    output.mkdir(parents=True, exist_ok=False)
    # An explicitly development-named archive; no stable/nightly or version-prefix upload.
    archive = output / 'chentu-preview.tar.gz'
    with archive.open('xb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', filename='', mtime=0) as stream:
        with tarfile.open(fileobj=stream, mode='w', format=tarfile.PAX_FORMAT) as target:
            for name, (data, mode) in sorted(files.items()):
                info = tarfile.TarInfo('chentu/' + name)
                info.mode, info.size, info.mtime = mode, len(data), 0
                target.addfile(info, io.BytesIO(data))
    identity = package.identity(archive)
    key = 'apeiron/blobs/sha256/' + identity['sha256']
    preview = {'schemaVersion': 1, 'kind': 'apeiron-development-preview',
               'distro': catalog['distro'], 'profiles': list(catalog['targets']),
               'sources': manifest['sources'], 'acceptance': 'not-yet-accepted',
               'catalogRepairs': repairs,
               'archive': {'url': package.PUBLIC + key, **identity},
               'resourceCount': len(catalog['files']),
               'resourceBytes': sum(item['size'] for item in catalog['files'])}
    (output / 'preview.json').write_text(json.dumps(preview, indent=2) + '\n')
    (output / 'upload-plan.json').write_text(json.dumps({'objects': {
        key: {'path': str(archive.resolve()), **identity}}}, indent=2) + '\n')
    return preview


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(prepare(args.profile.resolve(), args.out.resolve())))
