import { expect, test } from 'bun:test';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startInitServer } from './rust-server';
const id = 'a'.repeat(32);
const descriptor = { id, version: 1, identity: 'apeiron-123456789abc', domain: 'team.example.com', publicIp: '8.8.8.8',
    host: 'edge.example.com', sshPort: 22, tunnelPort: 19444, hostKey: 'public-host-key', deviceKey: 'must-not-reach-browser', inviteKey: 'must-not-reach-browser' };
test('pairing endpoint enforces origin, fixed body and never returns credentials in config or response', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'apeiron-pairing-api-'));
    await mkdir(join(directory, 'bootstrap'));
    const stub = `import sys,json,pathlib
request=json.load(sys.stdin)
assert set(request)=={'code'}
assert request['code']=='apeiron-pair-v1.fixture'
value=json.loads(${JSON.stringify(JSON.stringify(descriptor))})
folder=pathlib.Path(sys.argv[2])/value['id']
folder.mkdir(parents=True,exist_ok=True)
(folder/'connection.json').write_text(json.dumps(value))
print(json.dumps(value))
`;
    await writeFile(join(directory, 'bootstrap/public_pairing.py'), stub);
    const server = await startInitServer({ path: join(directory, 'config.json'), env: { APEIRON_CHENTU_ROOT: directory } });
    const post = (body, origin = server.origin) => fetch(server.url + 'api/connections/pair', { method: 'POST', headers: { origin, 'content-type': 'application/json' }, body: JSON.stringify(body) });
    try {
        expect((await post({ code: 'apeiron-pair-v1.fixture' }, 'https://untrusted.example')).status).toBe(403);
        expect((await post({ code: 'apeiron-pair-v1.fixture', command: 'id' })).status).toBe(400);
        const response = await post({ code: 'apeiron-pair-v1.fixture' });
        expect(response.status).toBe(200);
        const body = await response.text();
        expect(body).toContain(descriptor.domain);
        expect(body).not.toContain('must-not-reach-browser');
        const config = await (await fetch(server.url + 'api/config')).text();
        expect(config).toContain(id);
        expect(config).not.toContain('must-not-reach-browser');
        expect(config).not.toContain('public-host-key');
    }
    finally {
        await server.stop();
        await rm(directory, { recursive: true, force: true });
    }
});
