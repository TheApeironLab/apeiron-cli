#!/usr/bin/env python3
"""Disposable local-k3d Matrix protocol acceptance for the compiled Chat CLI."""
import hashlib
import hmac
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid

IMAGE = 'ghcr.io/element-hq/synapse@sha256:38879c6039381b9b66a2adb11b92c63dd5f7ee6a443b98d1edf10ffe004e17e4'

def main():
    if len(sys.argv) != 3 or not Path(sys.argv[2]).is_absolute():
        raise SystemExit('Usage: smoke-chat.py <local-k3d-cluster> <absolute-cli-binary>')
    cluster, binary = sys.argv[1:]
    ns = 'chat-cli-' + uuid.uuid4().hex[:10]
    with tempfile.TemporaryDirectory(prefix='apeiron-chat-smoke-') as directory:
        root = Path(directory)
        kubeconfig = root / 'kubeconfig'
        kubeconfig.write_bytes(subprocess.check_output(['k3d', 'kubeconfig', 'get', cluster]))
        kubeconfig.chmod(0o600)
        kubectl = ['kubectl', '--kubeconfig', str(kubeconfig)]
        config = json.loads(subprocess.check_output(kubectl + ['config', 'view', '--minify', '-o', 'json']))
        assert config['clusters'][0]['cluster']['server'].startswith('https://127.0.0.1:'), 'Local cluster required'
        subprocess.run(kubectl + ['create', 'namespace', ns], check=True)
        forward = None
        try:
            secret = secrets.token_hex(32)
            cfg = f'''server_name: matrix.test
pid_file: /data/homeserver.pid
listeners:
  - port: 8008
    tls: false
    type: http
    resources:
      - names: [client]
database:
  name: sqlite3
  args:
    database: /data/homeserver.db
media_store_path: /data/media
signing_key_path: /data/signing.key
report_stats: false
registration_shared_secret: {secret}
enable_registration: false
federation_domain_whitelist: []
'''
            objects = [
                {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': {'name': 'config'}, 'stringData': {'homeserver.yaml': cfg}},
                {'apiVersion': 'v1', 'kind': 'Pod', 'metadata': {'name': 'matrix'}, 'spec': {
                    'containers': [{'name': 'matrix', 'image': IMAGE,
                        'readinessProbe': {'httpGet': {'path': '/_matrix/client/versions', 'port': 8008}, 'periodSeconds': 1, 'failureThreshold': 60},
                        'env': [{'name': 'SYNAPSE_CONFIG_PATH', 'value': '/config/homeserver.yaml'}],
                        'volumeMounts': [{'name': 'config', 'mountPath': '/config'}, {'name': 'data', 'mountPath': '/data'}]}],
                    'volumes': [{'name': 'config', 'secret': {'secretName': 'config'}}, {'name': 'data', 'emptyDir': {}}]}}
            ]
            subprocess.run(kubectl + ['-n', ns, 'apply', '-f', '-'], input=json.dumps({'apiVersion': 'v1', 'kind': 'List', 'items': objects}), text=True, check=True)
            subprocess.run(kubectl + ['-n', ns, 'wait', '--for=condition=Ready', 'pod/matrix', '--timeout=180s'], check=True)
            log = root / 'forward.log'
            with log.open('w') as stream:
                forward = subprocess.Popen(kubectl + ['-n', ns, 'port-forward', 'pod/matrix', ':8008', '--address', '127.0.0.1'], stdout=stream, stderr=stream)
            for _ in range(100):
                text = log.read_text()
                if 'Forwarding from 127.0.0.1:' in text:
                    port = text.split('Forwarding from 127.0.0.1:')[1].split()[0]
                    break
                time.sleep(.1)
            else:
                raise RuntimeError('Port-forward unavailable')
            base = 'http://127.0.0.1:' + port
            def api(method, path, data=None, token=None):
                req = urllib.request.Request(base + path, method=method,
                    data=json.dumps(data).encode() if data is not None else None,
                    headers={'Content-Type': 'application/json', **({'Authorization': 'Bearer ' + token} if token else {})})
                with urllib.request.urlopen(req, timeout=15) as response:
                    return json.load(response)
            for _ in range(100):
                try:
                    api('GET', '/_matrix/client/versions')
                    break
                except OSError:
                    time.sleep(.1)
            else:
                raise RuntimeError('Matrix HTTP did not become ready')
            def register(name):
                nonce = api('GET', '/_synapse/admin/v1/register')['nonce']
                password = secrets.token_urlsafe(24)
                mac = hmac.new(secret.encode(), '\0'.join([nonce, name, password, 'notadmin']).encode(), hashlib.sha1).hexdigest()
                user = api('POST', '/_synapse/admin/v1/register', {'nonce': nonce, 'username': name, 'password': password, 'admin': False, 'mac': mac})
                path = root / name
                path.write_text(user['access_token'])
                path.chmod(0o600)
                return user
            alice, bob, outsider = [register(name) for name in ['alice', 'bob', 'outsider']]
            def cli(user, *args, expected=0):
                env = {**os.environ, 'APEIRON_CHAT_SERVER': base, 'APEIRON_CHAT_TOKEN_FILE': str(root / user)}
                result = subprocess.run([binary, 'chat', *args, '--jsonl'], env=env, capture_output=True, text=True, timeout=50)
                assert result.returncode == expected, (args, result.returncode, result.stderr)
                return json.loads(result.stdout) if expected == 0 else None
            assert cli('alice', 'whoami')['user_id'] == alice['user_id']
            room = cli('alice', 'room', 'create', '--user', bob['user_id'])['room_id']
            cli('bob', 'room', 'join', '--room', room)
            cli('alice', 'user', 'search', '--query', 'bob')
            sent = cli('alice', 'message', 'send', '--room', room, '--text', '你好，来自 apeiron chat CLI', '--txn-id', 'smoke-1')
            again = cli('alice', 'message', 'send', '--room', room, '--text', '你好，来自 apeiron chat CLI', '--txn-id', 'smoke-1')
            assert sent['event_id'] == again['event_id']
            history = cli('bob', 'message', 'list', '--room', room)
            assert sum(e['event_id'] == sent['event_id'] for e in history['items']) == 1
            cli('bob', 'room', 'read', '--room', room, '--event', sent['event_id'])
            cli('outsider', 'message', 'list', '--room', room, expected=7)
            cli('bob', 'sync', '--out', str(root / 'sync.json'))
            sync = json.loads((root / 'sync.json').read_text())
            reply = cli('bob', 'message', 'send', '--room', room, '--text', '收到', '--txn-id', 'reply-1')
            cli('bob', 'sync', '--cursor', sync['next_cursor'], '--out', str(root / 'sync-next.json'))
            events = json.loads((root / 'sync-next.json').read_text())['rooms']['join'][room]['timeline']['events']
            assert any(e['event_id'] == reply['event_id'] for e in events)
            cli('alice', 'message', 'edit', '--room', room, '--event', sent['event_id'], '--text', '修改后的消息', '--txn-id', 'edit-1')
            cli('alice', 'message', 'redact', '--room', room, '--event', sent['event_id'], '--txn-id', 'redact-1')
            from urllib.parse import quote
            api('PUT', '/_matrix/client/v3/rooms/' + quote(room, safe='') + '/state/m.room.encryption', {'algorithm': 'm.megolm.v1.aes-sha2'}, alice['access_token'])
            cli('alice', 'message', 'send', '--room', room, '--text', 'must not send', '--txn-id', 'blocked', expected=2)
            print(json.dumps({'schema': 'apeiron.chat.smoke.v1', 'passed': True, 'identity': True, 'send_retry_same_event': True,
                'history': True, 'incremental_sync': True, 'receipt': True, 'outsider_denied': True, 'edit_redact': True,
                'encrypted_send_refused': True, 'image': IMAGE}, indent=2))
        finally:
            if forward:
                forward.terminate()
                forward.wait(timeout=10)
            subprocess.run(kubectl + ['delete', 'namespace', ns, '--wait=false'], check=True)

if __name__ == '__main__':
    main()
