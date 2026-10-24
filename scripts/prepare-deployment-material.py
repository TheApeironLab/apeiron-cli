#!/usr/bin/env python3
"""Prepare a release from locked platform bytes and newly built application artifacts."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import copy
import gzip
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from distribution_scope import required_components, validate_catalog

HERE = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


files = load('distribution_files', HERE / 'package-distribution.py')


def run(args, **kwargs):
    return subprocess.check_output([str(a) for a in args], text=True, **kwargs).strip()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('release blob redirects are not accepted')


def download(record, output):
    if record['url'] != files.PUBLIC + 'apeiron/blobs/sha256/' + record['sha256']:
        raise ValueError('platform resources require content addressed public URLs')
    output.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.build_opener(NoRedirect).open(record['url'], timeout=900) as response, output.open('xb') as target:
        shutil.copyfileobj(response, target)
    if files.identity(output) != {key: record[key] for key in ('sha256', 'size')}:
        raise ValueError('downloaded platform bytes differ from the reviewed lock')
    mode = record.get('mode', 0o644)
    if mode not in (0o644, 0o755):
        raise ValueError('unsupported platform resource permissions')
    output.chmod(mode)


def archive_layout(layout, output):
    output.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(output, 'x') as archive:
        for path in sorted(layout.rglob('*')):
            if not path.is_file():
                continue
            files.identity(path)
            info = archive.gettarinfo(path, arcname=path.relative_to(layout))
            info.uid = info.gid = info.mtime = 0
            info.uname = info.gname = ''
            with path.open('rb') as stream:
                archive.addfile(info, stream)


def convert_application_image(archive, image_id, architecture, registry, reference, layout, hosted, native_images):
    """Convert through the existing loopback transport with a verified source identity."""
    if not registry.startswith('127.0.0.1:') or not registry.split(':')[1].isdigit():
        raise ValueError('image conversion registry must be loopback')
    identity, _ = hosted.image_archive_identity(archive, image_id, architecture)
    remote = f'{registry}/{reference}'
    run(['crane', 'push', '--insecure', archive, remote])
    digest = run(['crane', 'digest', '--insecure', remote])
    run(['crane', 'pull', '--insecure', '--format=oci', remote, layout])
    native_images.annotate(layout, reference, digest, architecture)
    manifest = json.loads((layout / 'blobs/sha256' / digest[7:]).read_text())
    if manifest['config']['digest'] != identity['configDigest']:
        raise ValueError('OCI image config differs from the measured source archive')
    config = json.loads((layout / 'blobs/sha256' / manifest['config']['digest'][7:]).read_text())
    return digest, config, identity


def render_values(values, arch, scope):
    text = json.dumps(values)
    for old, new in {'__REGISTRY__': '${REGISTRY}', '__BUNDLE__': '${BUNDLE_DIR}',
                     '__DOMAIN__': 'example.internal', '__ARCH__': arch}.items():
        text = text.replace(old, new)
    values = {name: item for name, item in json.loads(text).items() if item}
    if 'gpustack' in scope:
        # Helmfile needs an external endpoint to render GPUStack's Service.
        # This documentation address belongs only to the temporary render input;
        # the published catalog leaves endpoint selection to the installer.
        values.setdefault('gpustack', {}).update(gpuHost='192.0.2.20', gpuPort=18080)
    return values


def prepare(args):
    import yaml
    root = args.chentu.resolve()
    candidate = json.loads(args.candidate.read_text())
    version, revision = candidate['version'], candidate['sources']['chentu']
    if run(['git', '-C', root, 'rev-parse', 'HEAD']) != revision or run(['git', '-C', root, 'status', '--porcelain']):
        raise ValueError('Chentu must be clean at the candidate commit')
    sys.path.insert(0, str(root / 'cli/src'))
    from chentu.release_version import validate_version
    from chentu.local_computer import validate_material
    validate_version(version)
    assembler = load('release_assemble', root / 'bundle/assemble.py')
    install_catalog = load('release_install_catalog', root / 'bundle/install_catalog.py')
    hosted = load('release_hosted', root / 'bundle/hosted_release.py')
    native_images = load('release_native_images', root / 'bundle/native_images.py')
    recipe = json.loads(args.lock.read_text())
    args.out.mkdir(parents=True, exist_ok=False)
    seed_path = args.out / 'seed.json'
    download(recipe['platforms'][args.profile], seed_path)
    seed = json.loads(seed_path.read_text())
    if seed['schemaVersion'] != 2 or seed['kind'] != 'apeiron-platform-material' or seed['profile'] != args.profile:
        raise ValueError('wrong platform seed')
    # The catalog is generated below; its application scope is the reviewed Chentu declaration.
    scope = required_components(recipe, load('release_applications', root / 'cli/src/chentu/applications.py').load(root))
    material = args.out / 'material'
    material.mkdir()
    with ThreadPoolExecutor(max_workers=4) as executor:
        futures = [executor.submit(download, record, files.safe_file(material, name)) for name, record in seed['assets'].items()]
        for future in futures:
            future.result()
    # Regenerate source-derived operator files from the reviewed Chentu commit.
    # Downloaded materials never supply deployment source or source overlays.
    assembler.snapshot(root, revision, material)
    arch = files.PROFILES[args.profile]
    receipts, runtime, values = copy.deepcopy(seed['receipts']), copy.deepcopy(seed['runtime']), copy.deepcopy(seed['values'])
    toolbox = seed.get('toolboxLineage')
    if toolbox:
        archive = material / toolbox['export']['file']
        if files.identity(archive) != {k: toolbox['export'][k] for k in ('sha256', 'size')}:
            raise ValueError('toolbox lineage differs from its archive')
        run(['docker', 'load', '--input', archive])
        identifier = toolbox['derived']['imageId']
        if run(['docker', 'image', 'inspect', identifier, '--format', '{{.Architecture}}']) != arch:
            raise ValueError('toolbox architecture mismatch')
        bindir = args.out / 'bin'
        bindir.mkdir()
        for name in ('crane', 'helm', 'helmfile'):
            with (bindir / name).open('xb') as output:
                subprocess.run(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', 'cat', identifier, '/usr/local/bin/' + name], stdout=output, check=True)
            (bindir / name).chmod(0o755)
    else:
        bindir = material / 'bin'
    os.environ['PATH'] = str(bindir) + os.pathsep + os.environ['PATH']
    # Only a loopback registry is used to losslessly convert Docker-save builds
    # into OCI manifests. It is stopped by the caller's workflow cleanup step.
    if not args.registry.startswith('127.0.0.1:') or not args.registry.split(':')[1].isdigit():
        raise ValueError('image conversion registry must be loopback')
    for name in files.APEIRON_IMAGES:
        directory = args.images / f'candidate-{name}-{arch}'
        proof = json.loads((directory / 'build-provenance.json').read_text())
        archive = files.safe_file(directory, proof['archive']['file'])
        if (proof['version'] != version or proof['revision'] != candidate['sources']['apeiron']
                or proof['component'] != name or proof['architecture'] != arch
                or files.identity(archive) != {k: proof['archive'][k] for k in ('sha256', 'size')}):
            raise ValueError('application build provenance mismatch')
        plain = args.out / f'{name}.tar'
        with gzip.open(archive, 'rb') as source, plain.open('xb') as target:
            shutil.copyfileobj(source, target)
        reference = f'apeiron/{name}:{version}'
        layout = args.out / f'oci-{name}'
        digest, config, identity = convert_application_image(
            plain, proof['imageId'], arch, args.registry, reference, layout, hosted, native_images)
        labels = config['config']['Labels']
        if (labels['org.opencontainers.image.revision'] != proof['revision']
                or labels['org.opencontainers.image.version'] != version):
            raise ValueError('OCI image identity differs from the measured build')
        relative = f'images/apeiron_{name}_{version}.tar'
        archive_layout(layout, material / relative)
        plain.unlink()
        shutil.rmtree(layout)
        receipts.append({'components': ['apeiron'], 'file': relative, 'reference': reference, 'digest': digest,
                         'buildProvenance': {'source': {'repository': 'https://github.com/TheApeironLab/apeiron', 'commit': proof['revision'], 'clean': True},
                                             'recipe': {'file': proof['recipe']['path'], 'sha256': proof['recipe']['sha256']},
                                             'platform': 'linux/' + arch, 'archive': proof['archive'],
                                             'imageIdentity': {'sourceId': proof['imageId'], **identity}}})
    local = {'apeironVersion': 'v' + version, 'scodeVersion': recipe['sudocode']['version'], 'sourceRevision': candidate['sources']['apeiron']}
    shutil.copytree(args.clients / 'local-computer', material / 'local-computer', dirs_exist_ok=True)
    receipts.append({'components': ['apeiron'], 'files': validate_material(material, local), 'localComputer': local})
    skills = candidate['sources']['systemSkills']
    if run(['git', '-C', args.skills, 'rev-parse', 'HEAD']) != skills or run(['git', '-C', args.skills, 'status', '--porcelain']):
        raise ValueError('System Skills checkout differs from the pinned clean source')
    (material / 'files').mkdir(exist_ok=True)
    heads = run(['git', '-C', args.skills, 'bundle', 'list-heads', args.skills_bundle]).splitlines()
    if heads != [skills + ' HEAD']:
        raise ValueError('Shared Skills bundle does not identify the pinned source')
    run(['git', '-C', args.skills, 'bundle', 'verify', args.skills_bundle])
    shutil.copy2(args.skills_bundle, material / 'files/system-skills.bundle')
    receipts.append({'components': ['apeiron'], 'files': ['files/system-skills.bundle']})
    # Apeiron's image roles and Skills revision come from this release's own builds.
    apeiron = values.setdefault('apeiron', {})
    apeiron['image'] = {name: f'__REGISTRY__/apeiron/{name}:{version}' for name in files.APEIRON_IMAGES}
    apeiron.setdefault('systemSkills', {})['revision'] = skills
    if args.profile.startswith('k3s-'):
        if not args.sandbox or not args.gnupghome or not args.signing_key:
            raise ValueError('native material requires the prepared Debian pool and formal signing key')
        sandbox = load('release_sandbox', root / 'bundle/sandbox_apt.py')
        sandbox.build(args.sandbox, material / 'sandbox-apt', version, args.gnupghome, args.signing_key, arch)
        receipts.append({'components': ['apeiron'], 'files': sorted(
            p.relative_to(material).as_posix() for p in (material / 'sandbox-apt').rglob('*') if p.is_file())})
        wheels = args.out / 'wheels'
        run(['uv', 'build', '--wheel', '--out-dir', wheels, root / 'cli'])
        wheel = next(wheels.glob('chentu-*.whl'))
        (material / 'cli').mkdir(exist_ok=True)
        shutil.copy2(wheel, material / 'cli' / wheel.name)
        runtime['files'].append('cli/' + wheel.name)
    # Native Helmfile remains the dependency graph. Build inventory from the
    # selected components; do not carry a hand-written release order in the CLI.
    publisher = {'schemaVersion': 'chentu.catalog.v1', 'version': version, 'architecture': arch,
                 'apps': {'apeiron': ['apeiron']}, 'required': scope, 'values': render_values(values, arch, scope),
                 'resources': []}
    work = args.out / 'render'
    work.mkdir()
    config = assembler.inst.Installation.model_validate({'release': {'version': version}, 'organization': {'slug': 'example'},
        'installation': {'id': 'release-build'}, 'deployment': {'topology': 'single-k3d' if toolbox else 'single-k3s',
        'architecture': arch, 'bundleDir': str(material), 'registry': assembler.PLACEHOLDER_REGISTRY},
        'nodes': [] if toolbox else [{'name': 'node-1', 'address': '192.0.2.10', 'internalIp': '192.0.2.10', 'role': 'control-plane',
                                      'sshUser': 'root', 'sshKeyRef': '/nonexistent/release-key'}],
        'access': {'domain': 'example.internal'}, 'apps': ['apeiron']})
    choices = assembler.inst.Catalog.model_validate(publisher)
    environment, resolved, selected, releases = assembler.inst.select_releases(config, choices, work, [])
    assembler.inst.write_yaml(environment, resolved)
    assembler.inst.helmfile_output(material / 'chentu', environment, 'template')
    inventory = {'releases': [r for r in releases if r['name'] in selected]}
    (args.out / 'inventory.yaml').write_text(yaml.safe_dump(inventory, sort_keys=False))
    # install.json is generated from the seed receipts, this release's receipts and Helmfile.
    releases = {name: {'values': values[name]} if name in values else {} for name in resolved['releases']}
    catalog = install_catalog.build(material, inventory, {'releases': releases}, receipts, runtime, None)
    catalog['sources'] = {'apeiron': {'version': version, 'commit': candidate['sources']['apeiron']},
                          'sudocode': recipe['sudocode'],
                          'systemSkills': {'version': 'git-' + skills[:12], 'commit': skills,
                                           'asset': {'name': 'system-skills.bundle', 'sha256': files.identity(material / 'files/system-skills.bundle')['sha256']}}}
    validate_catalog(catalog, scope)
    for component in catalog['components'].values():
        for image in component.get('images', []):
            try:
                hosted.image_build_provenance(image, arch)
            except (TypeError, ValueError) as error:
                raise ValueError('Invalid platform image provenance: ' + image['reference']) from error
    (material / 'chentu/setup').mkdir(exist_ok=True)
    (material / 'chentu/setup/install.json').write_text(json.dumps(catalog, indent=2) + '\n')
    if toolbox:
        (args.out / 'toolbox-lineage.json').write_text(json.dumps(toolbox, indent=2) + '\n')
    else:
        roots = yaml.safe_load((root / 'bundle/sources.yaml').read_text())['debs']['host_packages']
        command = [sys.executable, root / 'bundle/check_deb_closure.py', '--debs', material / 'debs', '--architecture', arch, '--receipt', args.out / 'host-deb-proof.json']
        for name in roots:
            command += ['--require', name]
        run(command)
    return {'material': str(material), 'inventory': str(args.out / 'inventory.yaml'), 'profile': args.profile, 'version': version}


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('chentu', 'candidate', 'lock', 'images', 'clients', 'skills', 'skills-bundle', 'out'):
        p.add_argument('--' + name, required=True, type=Path)
    p.add_argument('--profile', choices=files.PROFILES, required=True)
    p.add_argument('--registry', required=True)
    p.add_argument('--sandbox', type=Path)
    p.add_argument('--gnupghome', type=Path)
    p.add_argument('--signing-key')
    print(json.dumps(prepare(p.parse_args())))
