#!/usr/bin/env python3
"""Prepare Minimal material from pinned online resources and native Helmfile output.

This creates material for Chentu's existing hosted_release.py, not a published
release. It neither changes channels nor installs a cluster. All output is external.
"""
import argparse
import copy
from concurrent.futures import ThreadPoolExecutor
import importlib.util
import hashlib
import re
import tarfile
import json
from pathlib import Path
import shutil
import sys


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def selected_receipts(catalog, inventory):
    """Resource ownership comes from the seed; deployment edges come from Helmfile."""
    names = {r['name'] if r['name'] != 'cert-manager' else 'ingress'
             for r in inventory['releases']}
    receipts = []
    for name in sorted(names):
        component = catalog['components'][name]
        images = component.get('images', [])
        owned = set(component['files'])
        for image in images:
            if image['file'] not in owned:
                raise ValueError('image outside component resource ownership: ' + name)
            receipts.append({'components': [name], **copy.deepcopy(image)})
        local = component.get('values', {}).get('releases', {}).get(name, {}).get('values', {}).get('localComputer')
        local_files = sorted(p for p in owned if p.startswith('local-computer/'))
        if local:
            receipts.append({'components': [name], 'localComputer': local, 'files': local_files})
        others = owned - {i['file'] for i in images} - (set(local_files) if local else set())
        if others:
            receipts.append({'components': [name], 'files': sorted(others)})
    return receipts


def verify_cli_archive(path, release):
    expected = release.get('files', {}).get(path.name)
    if not expected or not path.is_file() or path.is_symlink():
        raise ValueError('CLI archive is not declared by the CI release manifest')
    checksum = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            checksum.update(chunk)
    digest = checksum.hexdigest()
    if expected.get('sha256') != digest or expected.get('size') != path.stat().st_size:
        raise ValueError('CLI archive differs from the CI release manifest')
    with tarfile.open(path, 'r:gz') as archive:
        if sorted(archive.getnames()) != ['README.md', 'apeiron', 'build-provenance.json'] or any(
                not member.isfile() or member.size > 256 * 1024 * 1024 for member in archive):
            raise ValueError('unexpected CLI archive members')
        metadata = json.load(archive.extractfile('build-provenance.json'))
        binary = archive.extractfile('apeiron').read()
    if (metadata.get('kind') != 'apeiron-cli-build'
            or metadata.get('version') != release.get('version')
            or metadata.get('source', {}).get('revision') != release.get('revision')
            or not re.fullmatch(r'[a-f0-9]{40}', release.get('revision', ''))
            or metadata != release.get('builds', {}).get(metadata.get('target'))
            or metadata.get('binary') != {'size': len(binary), 'sha256': hashlib.sha256(binary).hexdigest()}
            or not metadata.get('sudocode', {}).get('version')):
        raise ValueError('CLI provenance or embedded scode identity differs')
    return metadata


def prepare(args):
    import yaml
    root, out = args.chentu.resolve(), args.out.resolve()
    if root == out or root in out.parents:
        raise ValueError('output must stay outside the source checkout')
    sys.path.insert(0, str(root / 'cli/src'))
    from chentu.environment import helmfile_output
    generator = load('distro_install_catalog', root / 'bundle/install_catalog.py')
    scope = load('distro_delivery_closure', root / 'bundle/delivery_closure.py')
    transfer = load('distro_material_transfer', Path(__file__).with_name('prepare-deployment-material.py'))
    source = json.loads(args.catalog.read_text())
    catalog = source.get('catalogs', {}).get(args.profile, source)
    if args.profile not in catalog.get('targets', {}):
        raise ValueError('seed does not supply the requested platform')
    original = copy.deepcopy(catalog['targets'][args.profile])
    archives = list(dict.fromkeys([*args.cli_archive, *([args.management_cli] if args.management_cli else [])]))
    if archives and not args.cli_release:
        raise ValueError('CLI archives require the CI release.json manifest')
    release = json.loads(args.cli_release.read_text()) if archives else {}
    for archive in archives:
        verify_cli_archive(archive, release)
    if args.profile.startswith('k3s-') and not args.management_cli:
        raise ValueError('native Minimal requires a matching management CLI archive with scode')
    out.mkdir(parents=True, exist_ok=False)
    material = out / 'material'
    material.mkdir()
    arch = args.profile.split('-')[-1]
    preset = yaml.safe_load((root / 'deploy/helmfile/environment.minimal.example.yaml').read_text())
    environment = copy.deepcopy(original['environment'])
    # Enable the committed distro preset. Helmfile remains responsible for
    # merging chart values and resolving needs; no second deployment graph.
    environment.pop('chartPins', None)
    environment.pop('nexusFirst', None)
    environment['distro'] = 'minimal'
    for name, settings in environment['releases'].items():
        settings['enabled'] = preset['releases'].get(name, {}).get('enabled', False)
    for name, settings in preset['releases'].items():
        environment['releases'].setdefault(name, {})['enabled'] = settings.get('enabled', False)
    environment['releases']['apeiron']['values']['models'] = {'deferred': True}
    render = copy.deepcopy(environment)
    render.update(topology='single-k3d' if args.profile.startswith('k3d-') else 'single-k3s',
                  architecture=arch, domain='example.internal', tenantSlug='example',
                  registry='registry.example.internal', node='server-1',
                  bundle=str(material), work=str(out / 'state'))
    encoded = json.dumps(render)
    for old, new in {'__REGISTRY__': render['registry'], '__DOMAIN__': render['domain'],
                     '__BUNDLE__': str(material), '__WORK__': str(out / 'state'),
                     '__ARCH__': arch, '__NODE__': 'server-1'}.items():
        encoded = encoded.replace(old, new)
    render_path = out / 'environment.yaml'
    render_path.write_text(yaml.safe_dump(json.loads(encoded), sort_keys=False))
    inventory = helmfile_output(root, render_path, 'build')
    (out / 'inventory.yaml').write_text(yaml.safe_dump(inventory, sort_keys=False))
    delivered = {r['name'] for r in inventory['releases']}
    for name in environment['releases']:
        if name not in delivered:
            environment['releases'][name] = {'enabled': False}
    receipts = selected_receipts(catalog, inventory)
    runtime = {k: v for k, v in original.items()
               if k not in ('environment', 'nexusFirst', 'managementCli')}
    runtime.update(target=args.profile, files=catalog['components']['tools']['files'])
    wanted = set(runtime['files'])
    for receipt in receipts:
        wanted.update(receipt.get('files', []))
        if 'file' in receipt:
            wanted.add(receipt['file'])
    entries = {f['path']: f for f in catalog['files']}
    if wanted - entries.keys():
        raise ValueError('seed is missing selected resource identities')
    # Paths and HTTPS content addresses are checked by the shared publisher
    # downloader; cached bytes are rehashed before reuse.
    with ThreadPoolExecutor(max_workers=6) as executor:
        jobs = [executor.submit(transfer.download, entries[name],
                                transfer.files.safe_file(material, name), args.cache)
                for name in sorted(wanted)]
        for index, job in enumerate(jobs, 1):
            job.result()
            print(f'verified resource {index}/{len(jobs)}', flush=True)
    # CLI archives are raw hosted resources, distinct from Apeiron app clients.
    for archive in args.cli_archive:
        relative = 'tools/' + archive.name
        (material / 'tools').mkdir(exist_ok=True)
        shutil.copyfile(archive, material / relative)
        receipts.append({'components': ['apeiron'], 'files': [relative]})
    if args.management_cli:
        relative = 'tools/' + args.management_cli.name
        (material / 'tools').mkdir(exist_ok=True)
        shutil.copyfile(args.management_cli, material / relative)
        runtime['managementCli'] = {'archive': relative}
    rendered = helmfile_output(root, render_path, 'template')
    generated = generator.build(material, inventory, environment, receipts, runtime, None, rendered=rendered)
    generated['sources'] = copy.deepcopy(catalog.get('sources', {}))
    # The named Minimal scope rejects Gateway, extra releases and missing needs.
    declaration = {name: ((root / name).read_bytes(), 0o644) for name in (
        scope.applications.PATH, 'deploy/helmfile/environment.minimal.example.yaml')}
    scope.delivery_scope(declaration, 'minimal', None, inventory, generated)
    destination = material / 'chentu/setup/install.json'
    destination.parent.mkdir(parents=True)
    destination.write_text(json.dumps(generated, indent=2) + '\n')
    (out / 'receipts.json').write_text(json.dumps(receipts, indent=2) + '\n')
    (out / 'runtime.json').write_text(json.dumps(runtime, indent=2) + '\n')
    print(json.dumps({'material': str(material), 'profile': args.profile,
                      'distro': 'minimal', 'resources': len(generated['files']),
                      'bytes': sum(f['size'] for f in generated['files'])}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('chentu', 'catalog', 'out', 'cache'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--profile', choices=('k3d-arm64', 'k3s-amd64', 'k3s-arm64'), required=True)
    parser.add_argument('--management-cli', type=Path)
    parser.add_argument('--cli-release', type=Path, help='CI release.json for all supplied CLI archives')
    parser.add_argument('--cli-archive', type=Path, action='append', default=[],
                        help='verified CI CLI archive to distribute through Nexus raw-hosted')
    prepare(parser.parse_args())
