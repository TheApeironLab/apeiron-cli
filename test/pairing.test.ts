import { expect, test } from 'bun:test';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PairingManager } from '../src/init/pairing';
import { startInitServer } from '../src/init/server';
import { validatePublicAccess } from '../src/init/installation';

const id = 'a'.repeat(32);
const descriptor = { id, version: 1, identity: 'apeiron-123456789abc', domain: 'team.example.com', publicIp: '8.8.8.8',
  host: 'edge.example.com', sshPort: 22, tunnelPort: 19444, hostKey: 'public-host-key', deviceKey: 'must-not-reach-browser', inviteKey: 'must-not-reach-browser' };

test('paired config accepts an opaque ID, excludes manual SSH and rejects forged IDs', () => {
  const access = { mode: 'relay' as const, publicIp: descriptor.publicIp, tunnelPort: 19444, pairingId: id };
  expect(validatePublicAccess(access)).toEqual(access);
  expect(() => validatePublicAccess({ ...access, pairingId: '../other' })).toThrow();
  expect(() => validatePublicAccess({ ...access, gateway: { host: 'elsewhere' } })).toThrow();
  expect(() => validatePublicAccess({ ...access, mode: 'direct' })).toThrow();
});

test('connection projection hides enrollment secrets and scope cannot cross teams or resurrect revoked entries', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-pairing-'));
  try {
    const manager = new PairingManager(join(directory, 'config.json'));
    const folder = join(manager.directory, id);
    await mkdir(folder, { recursive: true });
    await mkdir(join(manager.directory, 'b'.repeat(32))); // interrupted enrollment
    await writeFile(join(folder, 'connection.json'), JSON.stringify(descriptor));
    expect((await manager.list()).length).toBe(1);
    expect(JSON.stringify(await manager.list())).not.toContain('must-not-reach-browser');
    expect(JSON.stringify(await manager.list())).not.toContain('public-host-key');
    await expect(manager.scope(id, descriptor.domain, descriptor.publicIp, 19444)).resolves.toBeDefined();
    await expect(manager.scope(id, 'other.example.com', descriptor.publicIp, 19444)).rejects.toThrow('不匹配');
    await writeFile(join(folder, 'connection.json'), JSON.stringify({ ...descriptor, state: 'revoked' }));
    await expect(manager.scope(id, descriptor.domain, descriptor.publicIp, 19444)).rejects.toThrow('撤销');
    expect((await manager.action('revoke', id, new AbortController().signal)).state).toBe('revoked');
  } finally { await rm(directory, { recursive: true, force: true }); }
});

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
  const server = await startInitServer({ path: join(directory, 'config.json'), resources: async () => directory });
  const post = (body: object, origin = server.origin) => fetch(server.url + 'api/connections/pair', { method: 'POST', headers: { origin, 'content-type': 'application/json' }, body: JSON.stringify(body) });
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
  } finally { await server.stop(); await rm(directory, { recursive: true, force: true }); }
});
