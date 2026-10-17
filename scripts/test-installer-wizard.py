#!/usr/bin/env python3
"""Maintainer smoke check inside an isolated Ubuntu container; never deploys.

Mount the same staged installer read-only at two paths and give this process a
fresh HOME. Pass PLAN PACKAGE_PATH RELOCATED_PATH PROOF_PATH. The caller supplies
container isolation; this script only GETs wizard endpoints and runs local tools.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time
import urllib.request


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def run(args, **kwargs):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT,
                                   timeout=180, **kwargs)


def verify_payload(plan, package):
    root = package / 'bundle'
    expected = {item['path']: item for item in plan['files']}
    if len(expected) != len(plan['files']):
        raise ValueError('Duplicate payload path in plan')
    actual = set()
    for path in root.rglob('*'):
        mode = path.lstat().st_mode
        if stat.S_ISDIR(mode):
            continue
        relative = path.relative_to(root).as_posix()
        if not stat.S_ISREG(mode) or path.is_symlink():
            raise ValueError('Non-regular payload: ' + relative)
        actual.add(relative)
        item = expected.get(relative)
        if item is None or path.stat().st_size != item['size'] or sha(path) != item['sha256']:
            raise ValueError('Payload does not match plan: ' + relative)
    if actual != set(expected):
        raise ValueError('Missing payload files')


def check(plan, package):
    if not os.statvfs(package).f_flag & os.ST_RDONLY:
        raise ValueError('Smoke test requires a read-only package mount')
    verify_payload(plan, package)
    bindings = plan['bindings']
    assert sha(package / 'start.sh') == bindings['startScriptSha256']
    assert sha(package / 'bin/apeiron') == bindings['cliSha256']
    assert sha(package / 'provenance/default-config.json') == bindings['defaultConfigSha256']
    info = json.loads(run([str(package / 'bin/apeiron'), '--build-info']))
    assert info['revision'] == plan['cliProvenance']['source']['revision']
    state = Path.home() / '.apeiron/installers' / plan['candidate'] / 'instances/default'
    with tempfile.TemporaryFile(mode='w+') as log:
        process = subprocess.Popen(['bash', str(package / 'start.sh'), '--no-open'],
                                   stdout=log, stderr=subprocess.STDOUT)
        try:
            url = None
            for _ in range(120):
                log.seek(0)
                match = re.search(r'^url\t(http://127\.0\.0\.1:\d+/[^\s]+)',
                                  log.read(), re.M)
                if match:
                    url = match[1]
                    break
                if process.poll() is not None:
                    log.seek(0)
                    diagnostic = re.sub(r'http://[^\s]+', '[wizard URL]', log.read())
                    raise RuntimeError('Wizard exited before listening: ' + diagnostic[-2000:])
                time.sleep(0.5)
            assert url, 'Wizard did not listen within timeout'
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            with opener.open(url, timeout=10) as response:
                page = response.read()
                assert response.status == 200 and b'<html' in page.lower()
            with opener.open(url + 'api/config', timeout=10) as response:
                configuration = json.load(response)
            with opener.open(url + 'api/deployment', timeout=10) as response:
                deployment = json.load(response)
            assert deployment.get('phase') in ('idle', 'not-started'), deployment.get('phase')
            saved = json.loads((state / 'config.json').read_text())
            # The CLI configuration on disk is a direct schema-v2 config.
            assert saved['deployment']['offline'] is True
            assert saved['deployment']['bundleDir'] == str(package / 'bundle')
            assert saved['apps'] == plan['profile']['config']['apps']
            assert configuration['config']['deployment']['offline'] is True
            assert configuration['config']['deployment']['bundleDir'] == str(package / 'bundle')
            assert set(configuration['config']['apps']) == set(saved['apps'])
            assert configuration['config']['deployment']['installation']['nodes'] == saved['deployment']['installation']['nodes']
            assert (state / 'config.json').stat().st_mode & 0o777 == 0o600
            return info, {'page': True, 'configuration': True, 'deploymentPhase': deployment['phase'],
                          'packagePath': str(package), 'stateInHome': True,
                          'readOnlyMount': True, 'payloadMatchesPlan': True,
                          'configurationMode': '0600', 'pageSha256': hashlib.sha256(page).hexdigest()}
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def main():
    if not __debug__:
        raise RuntimeError('Do not disable smoke assertions with Python -O')
    plan_path, first, relocated, proof_path = map(Path, sys.argv[1:])
    plan = json.loads(plan_path.read_text())
    if first == relocated or not first.is_absolute() or not relocated.is_absolute():
        raise ValueError('Two different absolute mount paths are required')
    if (Path.home() / '.apeiron/installers' / plan['candidate']).exists():
        raise ValueError('Smoke test requires fresh candidate state under HOME')
    info, before = check(plan, first)
    _, after = check(plan, relocated)
    state = Path.home() / '.apeiron/installers' / plan['candidate']
    python = state / 'bootstrap/python/bin/python3.12'
    bundle = relocated / 'bundle'
    env = {**os.environ, 'UV_OFFLINE': '1', 'UV_CACHE_DIR': str(state / 'cache/uv'),
           'PYTHONPYCACHEPREFIX': str(state / 'cache/python'), 'PYTHONDONTWRITEBYTECODE': '1'}
    venv = state / 'operator-tools'
    run([str(bundle / 'bin/uv'), 'venv', '--python', str(python), str(venv)], env=env)
    run([str(bundle / 'bin/uv'), 'pip', 'install', '--python', str(venv / 'bin/python'),
         '--no-index', '--find-links', str(bundle / 'cli'), 'chentu',
         '-r', str(bundle / 'chentu/bootstrap/requirements.txt')], env=env)
    versions = {}
    for tool, args in [('helm', ['version', '--short']), ('helmfile', ['--version']),
                       ('age', ['--version']), ('age-keygen', ['--version']),
                       ('s5cmd', ['version']), ('uv', ['--version'])]:
        versions[tool] = run([str(bundle / 'bin' / tool), *args], env=env).strip()
    if plan['contract']['contract'] == 'hosted-first':
        versions['crane'] = run([str(bundle / 'bin/crane'), 'version'], env=env).strip()
    versions['k3s'] = run([str(bundle / 'k3s/k3s'), '--version'], env=env).strip()
    versions['python'] = run([str(python), '--version'], env=env).strip()
    versions['ansible'] = run([str(venv / 'bin/ansible'), '--version'], env=env).splitlines()[0]
    run([str(venv / 'bin/chentu'), '--help'], env=env)
    # A read-only mount prevents these runs from altering the checked payload.
    proof = {'status': 'passed', 'page': True, 'configuration': True,
             'bindings': plan['bindings'], 'buildInfo': info, 'runs': [before, after],
             'requests': ['GET page', 'GET api/config', 'GET api/deployment'],
             'relocation': {'status': 'passed', 'paths': [str(first), str(relocated)]},
             'homeState': {'status': 'passed', 'isolatedCandidate': plan['candidate']},
             'runtimeToolExecution': {'status': 'passed', 'versions': versions},
             'pythonDependencyInstallation': {'status': 'passed', 'offline': True,
                                             'requirements': 'chentu + bootstrap/requirements.txt'},
             'actualDeployment': 'not-run', 'login': 'not-run', 'business': 'not-run'}
    with proof_path.open('x') as output:
        json.dump(proof, output, indent=2)
        output.write('\n')
    print('Wizard page/configuration, relocation, home state and offline tools passed')


if __name__ == '__main__':
    main()
