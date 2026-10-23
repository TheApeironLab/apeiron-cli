#!/usr/bin/env python3
"""Run native acceptance on tagged, four-hour ECS leases via Cloud Assistant.

No persistent GitHub runner token or public SSH rule is needed. Only the bounded
acceptance receipt returns to Actions; configuration and logs stay on the lease.
"""
import argparse
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tarfile
import time

PURPOSE = 'apeiron-release-acceptance'
BUCKET = 'apeiron-bj-cli-downloads'
TARGETS = {'k3s-single-amd64': 1, 'k3s-multi-amd64': 3}


def run(args, **kwargs):
    kwargs.setdefault('timeout', 900)
    result = subprocess.run([str(x) for x in args], capture_output=True, text=True, **kwargs)
    if result.returncode:
        # Command arguments/output may contain a signed URL or generated SSH key.
        raise RuntimeError(str(args[0]) + ' operation failed; inspect the private job state')
    return result.stdout.strip()


def api(action, **parameters):
    command = ['aliyun', 'ecs', action]
    for name, value in parameters.items():
        if isinstance(value, (dict, list)):
            value = json.dumps(value)
        elif isinstance(value, bool):
            value = str(value).lower()
        command += ['--' + name, str(value)]
    return json.loads(run(command))


def validate_settings(settings):
    if set(settings) != {'region', 'vpcId', 'vSwitchId', 'securityGroupId', 'imageId'}:
        raise ValueError('Unexpected native lease configuration')
    if settings['region'] != 'cn-beijing' or not re.fullmatch(r'ubuntu_22_04_x64_[A-Za-z0-9_.]+', settings['imageId']):
        raise ValueError('Native acceptance requires the reviewed Ubuntu 22.04 AMD64 region/image')
    for key, prefix in [('vpcId', 'vpc-'), ('vSwitchId', 'vsw-'), ('securityGroupId', 'sg-')]:
        if not re.fullmatch(prefix + '[a-z0-9]+', settings[key]):
            raise ValueError('Invalid acceptance network identifier')
    return settings


def owned_instances(state, records):
    if len(records) > TARGETS[state['target']]:
        raise ValueError('Lease contains more instances than the approved topology')
    for item in records:
        tags = {x['TagKey']: x['TagValue'] for x in item.get('Tags', {}).get('Tag', [])}
        network = item.get('VpcAttributes', {})
        if (tags.get('purpose') != PURPOSE or tags.get('release-run') != state['lease']
                or item['InstanceType'] != 'ecs.u1-c1m4.2xlarge'
                or network.get('VpcId') != state['settings']['vpcId']
                or network.get('VSwitchId') != state['settings']['vSwitchId']
                or state['settings']['securityGroupId'] not in item.get('SecurityGroupIds', {}).get('SecurityGroupId', [])
                or item.get('ImageId') != state['settings']['imageId']
                or item.get('InstanceChargeType') != 'PostPaid'
                or not item.get('AutoReleaseTime')):
            raise ValueError('Refusing to operate on an instance outside this disposable lease')
        created = datetime.fromisoformat(item['CreationTime'].replace('Z', '+00:00'))
        expiry = datetime.fromisoformat(item['AutoReleaseTime'].replace('Z', '+00:00'))
        if not timedelta(0) < expiry - created <= timedelta(hours=4, minutes=5):
            raise ValueError('Instance expiry exceeds the bounded acceptance lease')
        if state.get('instances') and item['InstanceId'] not in state['instances']:
            raise ValueError('Unexpected instance identity in the lease')
    return records


def describe(state):
    result = api('DescribeInstances', RegionId=state['settings']['region'], PageSize=100,
                 **{'Tag.1.Key': 'purpose', 'Tag.1.Value': PURPOSE,
                    'Tag.2.Key': 'release-run', 'Tag.2.Value': state['lease']})
    return owned_instances(state, result['Instances']['Instance'])


def save(path, state):
    path.write_text(json.dumps(state, indent=2) + '\n')
    path.chmod(0o600)


def prepare(state_path, target, release, source):
    if target not in TARGETS:
        raise ValueError('Unknown native acceptance topology')
    run_id, attempt = os.environ['GITHUB_RUN_ID'], os.environ['GITHUB_RUN_ATTEMPT']
    if not all(re.fullmatch('[1-9][0-9]*', x) for x in [run_id, attempt]):
        raise ValueError('Lease requires exact Actions run identity')
    settings = validate_settings(json.loads(os.environ['ACCEPTANCE_ECS_SETTINGS']))
    lease = f'{run_id}-{attempt}-{target}'
    if state_path.exists():
        raise ValueError('Refusing to reuse a previous lease state')
    state_path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    state = {'schemaVersion': 1, 'lease': lease, 'target': target, 'settings': settings,
             'instances': [], 'keyPair': 'apeiron-acceptance-' + lease,
             'object': 'apeiron/acceptance-staging/' + lease + '/transport.tar.gz'}
    save(state_path, state)  # Cleanup can rediscover a timed-out RunInstances request.
    key = state_path.parent / 'id_ed25519'
    run(['ssh-keygen', '-t', 'ed25519', '-N', '', '-f', key])
    transport = state_path.parent / 'transport.tar.gz'
    version = json.loads((release / 'release.json').read_text())['version']
    with tarfile.open(transport, 'x:gz') as archive:
        archive.add(Path(shutil.which('bun')).resolve(), arcname='bun', recursive=False)
        for name in ['release.json', f'apeiron-{version}-linux-x64.tar.gz']:
            archive.add(release / name, arcname='release/' + name, recursive=False)
        for name in ['scripts/accept-distribution.js', 'scripts/lib/distribution.js', 'scripts/lib/release-version.js', 'release/distribution-input.json']:
            archive.add(source / name, arcname='code/' + name, recursive=False)
    transport_sha = hashlib.sha256(transport.read_bytes()).hexdigest()
    run(['aliyun', 'ossutil', 'api', 'put-object', '--bucket', BUCKET, '--key', state['object'], '--body', 'file://' + str(transport),
         '--forbid-overwrite', '--object-acl', 'private', '--region', settings['region']])
    signed = run(['aliyun', 'ossutil', 'presign', 'oss://' + BUCKET + '/' + state['object'], '--region', settings['region'], '--expires-duration', '2h'])
    urls = [line for line in signed.splitlines() if line.startswith('https://')]
    if len(urls) != 1:
        raise ValueError('Expected one private transport URL')
    api('ImportKeyPair', RegionId=settings['region'], KeyPairName=state['keyPair'], PublicKeyBody=Path(str(key) + '.pub').read_text())
    expires = (datetime.now(timezone.utc) + timedelta(hours=4)).strftime('%Y-%m-%dT%H:%M:%SZ')
    result = api('RunInstances', RegionId=settings['region'], Amount=TARGETS[target],
        InstanceType='ecs.u1-c1m4.2xlarge', ImageId=settings['imageId'],
        VSwitchId=settings['vSwitchId'], SecurityGroupId=settings['securityGroupId'],
        InstanceChargeType='PostPaid', InternetChargeType='PayByTraffic', InternetMaxBandwidthOut=10,
        KeyPairName=state['keyPair'], InstanceName='apeiron-acceptance-' + lease,
        AutoReleaseTime=expires, ClientToken=hashlib.sha256(lease.encode()).hexdigest(),
        **{'SystemDisk.Category': 'cloud_essd', 'SystemDisk.Size': 200,
           'Tag.1.Key': 'purpose', 'Tag.1.Value': PURPOSE, 'Tag.2.Key': 'release-run', 'Tag.2.Value': lease})
    state['instances'] = result['InstanceIdSets']['InstanceIdSet']
    save(state_path, state)
    deadline = time.monotonic() + 600
    while True:
        instances = describe(state)
        if len(instances) == TARGETS[target] and all(x['Status'] == 'Running' for x in instances):
            break
        if time.monotonic() > deadline:
            raise TimeoutError('Disposable ECS instances did not become ready')
        time.sleep(10)
    instances.sort(key=lambda x: x['InstanceId'])
    primary = instances[0]['InstanceId']
    while True:
        status = api('DescribeCloudAssistantStatus', RegionId=settings['region'], **{'InstanceId.1': primary})
        agents = status.get('InstanceCloudAssistantStatusSet', {}).get('InstanceCloudAssistantStatus', [])
        if agents and agents[0].get('CloudAssistantStatus') == 'true':
            break
        if time.monotonic() > deadline:
            raise TimeoutError('Cloud Assistant did not become ready')
        time.sleep(10)
    nodes = []
    for index, item in enumerate(instances):
        address = item['VpcAttributes']['PrivateIpAddress']['IpAddress'][0]
        if not ipaddress.ip_address(address).is_private:
            raise ValueError('Cluster nodes must use private test VPC addresses')
        nodes.append({'name': 'node-' + str(index + 1), 'host': address, 'address': address, 'role': 'server'})
    remote = '/opt/apeiron-acceptance/' + lease
    apps = json.loads((source / 'runtime/assets/apps.json').read_text())
    profile = {'schemaVersion': 1, 'slug': 'release-' + run_id,
        'apps': sorted({x['id'] for x in apps if x.get('required')} | {'apeiron'}),
        # This baseline exercises installation and IAM, never model inference.
        'models': {'provider': 'fixture', 'baseUrl': 'https://models.example.internal/v1',
                   'apiKey': 'fixture-only-key', 'fast': 'fixture-chat', 'deep': 'fixture-chat'},
        'deployment': {'offline': False, 'installation': {'topology': 'single-k3s' if len(nodes) == 1 else 'multi-k3s',
            'ha': len(nodes) == 3, 'domain': f'release-{run_id}-{attempt}-{len(nodes)}.apeironlab.internal',
            'entryIp': nodes[0]['address'], 'httpPort': 80, 'httpsPort': 443,
            'sshUser': 'root', 'sshPort': 22, 'sshKey': remote + '/id_ed25519', 'nodes': nodes}}}
    encode = lambda value: base64.b64encode(value.encode()).decode()
    content = f'''#!/bin/bash
set -Eeuo pipefail
umask 077
work={shlex.quote(remote)}
mkdir -p "$work"
exec 3>&1
exec >"$work/bootstrap.log" 2>&1
failure() {{
  code=$?
  trap - ERR
  printf 'APEIRON_DIAGNOSTIC=' >&3
  {{
    tail -c 3000 "$work/bootstrap.log"
    tail -c 3000 "$work/acceptance.log" 2>/dev/null || true
    find "$work/run" -maxdepth 4 -type f -name install.log -exec tail -c 6000 '{{}}' '+' 2>/dev/null | tail -c 6000 || true
  }} | base64 -w0 >&3
  printf '\\n' >&3
  exit "$code"
}}
trap failure ERR
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y ca-certificates curl tar openssh-client
curl --fail --silent --show-error --retry 3 --proto '=https' --max-time 900 {shlex.quote(urls[0])} -o "$work/transport.tar.gz"
printf '%s  %s\\n' {shlex.quote(transport_sha)} "$work/transport.tar.gz" | sha256sum -c -
tar -xzf "$work/transport.tar.gz" -C "$work"
printf '%s' {shlex.quote(encode(key.read_text()))} | base64 -d > "$work/id_ed25519"
printf '%s' {shlex.quote(encode(json.dumps(profile)))} | base64 -d > "$work/profile.json"
chmod 600 "$work/id_ed25519" "$work/profile.json"
chmod 755 "$work/bun"
export GITHUB_RUN_ID={run_id} GITHUB_RUN_ATTEMPT={attempt} APEIRON_RELEASE_DISPOSABLE=true
export APEIRON_ACCEPTANCE_CONFIG="$work/profile.json"
"$work/bun" "$work/code/scripts/accept-distribution.js" {target} "$work/release" "$work/code/release/distribution-input.json" "$work/run" >"$work/acceptance.log" 2>&1
printf 'APEIRON_RECEIPT=' >&3
base64 -w0 "$work/run/{target}.json" >&3
printf '\\n' >&3
'''
    encoded = encode(content)
    if len(encoded) > 24 * 1024:
        raise ValueError('Cloud Assistant command exceeds its bounded payload')
    invocation = api('RunCommand', RegionId=settings['region'], Type='RunShellScript', CommandContent=encoded,
                     ContentEncoding='Base64', KeepCommand=False, Timeout=6600, **{'InstanceId.1': primary})
    state['invocation'] = invocation['InvokeId']
    save(state_path, state)
    print('Created disposable ' + target + ' lease; automatic expiry: ' + expires, flush=True)
    deadline = time.monotonic() + 6700
    while True:
        result = api('DescribeInvocationResults', RegionId=settings['region'], InvokeId=state['invocation'], ContentEncoding='Base64')
        records = result.get('Invocation', {}).get('InvocationResults', {}).get('InvocationResult', [])
        for record in records:
            if record.get('InstanceId') != primary:
                raise ValueError('Unexpected Cloud Assistant execution identity')
            status = record.get('InvocationStatus')
            if status == 'Success':
                output = base64.b64decode(record['Output']).decode()
                lines = [line.removeprefix('APEIRON_RECEIPT=') for line in output.splitlines() if line.startswith('APEIRON_RECEIPT=')]
                if len(lines) != 1:
                    raise ValueError('Acceptance did not return exactly one bounded receipt')
                receipt = json.loads(base64.b64decode(lines[0]))
                if receipt.get('runId') != run_id or receipt.get('target') != target:
                    raise ValueError('Remote receipt belongs to another test')
                (state_path.parent / (target + '.json')).write_text(json.dumps(receipt, indent=2) + '\n')
                return
            if status in {'Failed', 'Timeout', 'Cancelled', 'Stopped', 'Invalid', 'Aborted', 'Error', 'Terminated'}:
                output = base64.b64decode(record.get('Output', '')).decode(errors='replace')
                diagnostics = [line.removeprefix('APEIRON_DIAGNOSTIC=') for line in output.splitlines()
                               if line.startswith('APEIRON_DIAGNOSTIC=')]
                if len(diagnostics) == 1:
                    log = state_path.parent / 'private-diagnostic.txt'
                    log.write_bytes(base64.b64decode(diagnostics[0])[:12000])
                    log.chmod(0o600)
                    diagnostic_key = state['object'].replace('transport.tar.gz', 'diagnostic.txt')
                    run(['aliyun', 'ossutil', 'api', 'put-object', '--bucket', BUCKET, '--key', diagnostic_key,
                         '--body', 'file://' + str(log), '--forbid-overwrite', '--object-acl', 'private',
                         '--region', settings['region']])
                    print('Private bounded failure log retained at oss://' + BUCKET + '/' + diagnostic_key, flush=True)
                raise RuntimeError('Native installation failed; no acceptance receipt issued')
        if time.monotonic() > deadline:
            raise TimeoutError('Native installation exceeded its lease test deadline')
        time.sleep(15)


def cleanup(path):
    if not path.exists():
        return
    state = json.loads(path.read_text())
    settings = validate_settings(state['settings'])
    if not re.fullmatch(r'[1-9][0-9]*-[1-9][0-9]*-k3s-(single|multi)-amd64', state['lease']):
        raise ValueError('Invalid cleanup lease identity')
    if state['target'] not in TARGETS or not state['lease'].endswith('-' + state['target']):
        raise ValueError('Cleanup topology differs from the lease')
    if state['keyPair'] != 'apeiron-acceptance-' + state['lease'] or state['object'] != 'apeiron/acceptance-staging/' + state['lease'] + '/transport.tar.gz':
        raise ValueError('Cleanup resources differ from the owned lease')
    failures = []
    for item in describe(state):
        try:
            api('DeleteInstance', RegionId=settings['region'], InstanceId=item['InstanceId'], Force=True)
        except Exception:
            failures.append('instance ' + item['InstanceId'])
    deadline = time.monotonic() + 180
    while not failures and describe(state):
        if time.monotonic() > deadline:
            failures.append('instance release confirmation')
            break
        time.sleep(5)
    try:
        api('DeleteKeyPairs', RegionId=settings['region'], KeyPairNames=[state['keyPair']])
    except Exception:
        failures.append('key pair')
    try:
        run(['aliyun', 'ossutil', 'api', 'delete-object', '--bucket', BUCKET, '--key', state['object'], '--region', settings['region']])
    except Exception:
        failures.append('private transport')
    if failures:
        raise RuntimeError('Lease cleanup needs attention: ' + ', '.join(failures))
    print('Removed disposable native instances, key pair and private transport')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['run', 'cleanup'])
    parser.add_argument('--state', type=Path, required=True)
    parser.add_argument('--target', choices=TARGETS)
    parser.add_argument('--release', type=Path)
    parser.add_argument('--source', type=Path)
    args = parser.parse_args()
    if args.mode == 'cleanup':
        cleanup(args.state.resolve())
    else:
        if (os.environ.get('GITHUB_REPOSITORY') != 'TheApeironLab/apeiron-cli'
                or not os.environ.get('GITHUB_REF', '').startswith('refs/tags/v')):
            parser.error('native leases may only be created by the tagged release workflow')
        if not all([args.target, args.release, args.source]):
            parser.error('run requires target, release and source')
        prepare(args.state.resolve(), args.target, args.release.resolve(), args.source.resolve())
