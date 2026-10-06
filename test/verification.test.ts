import { expect, test } from 'bun:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { decodeInitialAdmin, readInitialAdmin, verifyInstallation } from '../src/init/verification';
import { installationDefaults } from '../src/init/installation';
import { startInitServer } from '../src/init/server';
import { deploymentFixture, freshInstallFixture, requiredApps } from './fixtures';

const credentials = { username: 'fixture-admin', password: 'not-a-real-password<&"' };
const secret = { metadata: { name: 'keycloak-bootstrap', namespace: 'keycloak' },
  data: { ...Object.fromEntries(Object.entries(credentials).map(([key, value]) => [key, Buffer.from(value).toString('base64')])), extra: Buffer.from('must-not-return').toString('base64') } };
const access = { domain: 'example.internal', entryIp: '127.0.0.1', local: true, notes: [] };

test('credentials decoder returns only username/password from the expected Secret and redacts malformed output', () => {
  expect(decodeInitialAdmin(JSON.stringify(secret))).toEqual(credentials);
  for (const bad of [credentials.password, '{}', JSON.stringify({ ...secret, metadata: { ...secret.metadata, namespace: 'elsewhere' } }),
    JSON.stringify({ ...secret, data: { ...secret.data, password: 'invalid base64' } }),
    JSON.stringify({ ...secret, data: { ...secret.data, username: Buffer.from('bad\nuser').toString('base64') } })]) {
    try { decodeInitialAdmin(bad); throw new Error('unexpected success'); }
    catch (error) { expect((error as Error).message).toBe('暂时无法读取初始管理员凭据，请确认集群可连接后重试。'); }
  }
});

test('credential reader pins the current kubeconfig and Secret; Docker has no socket, writes, pulls or credential argv', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-admin-read-'));
  const setup = await deploymentFixture(dir);
  const target = { ...setup.target, installation: { ...installationDefaults(), topology: 'single-k3d' as const, domain: access.domain, entryIp: access.entryIp } };
  try {
    await writeFile(join(setup.workDir, 'installation.json'), JSON.stringify({ cluster: 'apeiron-fixture', domain: access.domain }));
    let calls = 0;
    const run = async (command: string, args: string[]) => {
      calls++; expect(command).toBe('docker');
      expect(args).toContain('--pull=never'); expect(args).toContain('k3d-apeiron-fixture');
      expect(args).toContain(join(setup.workDir, 'state/kubeconfig') + ':/kubeconfig:ro');
      expect(args.slice(-7)).toEqual(['-n', 'keycloak', 'get', 'secret', 'keycloak-bootstrap', '-o', 'json']);
      expect(args.join(' ')).not.toContain('docker.sock'); expect(args.join(' ')).not.toContain(credentials.password);
      return JSON.stringify(secret);
    };
    expect(await readInitialAdmin(target, new AbortController().signal, run)).toEqual(credentials);
    expect(calls).toBe(1);
    await writeFile(join(setup.workDir, 'installation.json'), JSON.stringify({ cluster: 'apeiron-fixture', domain: 'wrong.internal' }));
    await expect(readInitialAdmin(target, new AbortController().signal, run)).rejects.toThrow('暂时无法读取'); expect(calls).toBe(1);
    await expect(readInitialAdmin({ ...target, runner: 'native', kubeconfig: '/generated/kubeconfig' }, new AbortController().signal,
      async (command, args) => { expect(command).toBe('kubectl'); expect(args.slice(0, 2)).toEqual(['--kubeconfig', '/generated/kubeconfig']); throw new Error(credentials.password); })).rejects.toThrow('暂时无法读取');
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('verification rejects wrong DNS, TLS errors and HTTP 404; only expected core hosts are contacted', async () => {
  const contacted: string[] = [];
  const good = { resolveHost: async () => [access.entryIp], checkHttps: async (host: string, ip: string) => {
    expect(ip).toBe(access.entryIp); contacted.push(host); return 302;
  } };
  const result = await verifyInstallation(access, new AbortController().signal, good);
  expect(result.passed).toBe(true); expect(contacted.sort()).toEqual(['apeiron.example.internal', 'iam.example.internal', 'ops.example.internal']);
  const badDns = await verifyInstallation(access, new AbortController().signal, { ...good, resolveHost: async () => [access.entryIp, '192.0.2.1'] });
  expect(badDns.passed).toBe(false); expect(badDns.checks.every(check => check.https === 'skipped')).toBe(true); expect(contacted).toHaveLength(3);
  const failures = await verifyInstallation(access, new AbortController().signal, { resolveHost: async host => { if (host.startsWith('iam.')) throw new Error(); return [access.entryIp]; },
    checkHttps: async host => { if (host.startsWith('apeiron.')) throw new Error('TLS failure with private diagnostic'); return 404; } });
  expect(failures.passed).toBe(false); expect(failures.checks.find(check => check.name === 'Apeiron Ops')?.httpStatus).toBe(404);
  expect(JSON.stringify(failures)).not.toContain('private diagnostic');
  const controller = new AbortController(); controller.abort();
  await expect(verifyInstallation(access, controller.signal, good)).rejects.toThrow();
});

test('credential endpoint requires a successful deployment, same-origin POST and no parameters; general responses/logs never contain passwords', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-verification-api-'));
  const setup = await deploymentFixture(dir);
  await freshInstallFixture(dir, setup, 0, access.domain);
  const oldPath = process.env.PATH; process.env.PATH = setup.bin + ':' + oldPath;
  const server = await startInitServer({ path: join(dir, 'config.json'), resources: async () => setup.root,
    verify: async (info, signal) => verifyInstallation(info, signal, { resolveHost: async () => [info.entryIp], checkHttps: async () => 200 }) });
  const post = (endpoint: string, body: unknown = {}, origin = server.origin) => fetch(server.url + 'api/' + endpoint,
    { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  try {
    expect((await post('credentials')).status).toBe(409);
    expect((await fetch(server.url + 'api/credentials')).status).toBe(404);
    expect((await post('credentials', {}, 'https://untrusted.example')).status).toBe(403);
    expect((await post('verification')).status).toBe(409);
    expect((await post('deploy', { slug: 'fixture', apps: requiredApps, revision: null, deployment: { offline: false,
      installation: { ...installationDefaults(), topology: 'single-k3d', httpPort: 54320, httpsPort: 54321, domain: access.domain, entryIp: access.entryIp } } })).status).toBe(202);
    for (let i = 0; i < 400 && server.result.phase !== 'succeeded' && server.result.phase !== 'failed'; i++) await Bun.sleep(10);
    expect(server.result.phase).toBe('succeeded');
    expect((await post('credentials', { path: '/anywhere', secret: 'other' })).status).toBe(400);
    expect((await post('verification', { domain: 'attacker.internal' })).status).toBe(400);
    const response = await post('credentials');
    expect(response.status).toBe(200); expect(response.headers.get('cache-control')).toBe('no-store');
    const admin = await response.json(); expect(admin).toEqual({ username: 'fixture-admin', password: 'fixture-only-admin-password' });
    for (const endpoint of ['config', 'deployment', 'access', 'verification', 'log']) {
      expect(await fetch(server.url + 'api/' + endpoint).then(r => r.text())).not.toContain(admin.password);
    }
    expect(await readFile(server.result.log!, 'utf8')).not.toContain(Buffer.from(admin.password).toString('base64'));
    const checked = await post('verification').then(r => r.json()); expect(checked.result.passed).toBe(true);
    expect((await fetch(server.url + 'api/verification').then(r => r.json())).result).toEqual(checked.result);
  } finally { await server.stop(); process.env.PATH = oldPath; await rm(dir, { recursive: true, force: true }); }
}, 10_000);


test('HTTPS verification carries the chosen public port without changing DNS hostnames', async () => {
  const seen: number[] = [];
  const result = await verifyInstallation({ ...access, httpsPort: 54321 }, new AbortController().signal, {
    resolveHost: async host => { expect(host).not.toContain(':'); return [access.entryIp]; },
    checkHttps: async (_host, _ip, _signal, port) => { seen.push(port!); return 200; },
  });
  expect(seen).toEqual([54321, 54321, 54321]);
  expect(result.checks.every(check => new URL(check.url).port === '54321')).toBe(true);
  await expect(verifyInstallation({ ...access, httpsPort: 65536 }, new AbortController().signal)).rejects.toThrow('端口');
});
