import { afterAll, beforeAll, expect, test } from 'bun:test';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { type AccessArtifacts, publicCa } from '../src/init/access';
import { LocalAccessInstaller, macAccessScript, macAuthorizationScript, mergeHosts } from '../src/init/local-access';
import { startInitServer } from '../src/init/server';
import { installationDefaults } from '../src/init/installation';
import { deploymentFixture, freshInstallFixture, requiredApps } from './fixtures';

const exec = promisify(execFile);
let directory: string;
let artifacts: AccessArtifacts;
beforeAll(async () => {
  directory = await mkdtemp(join(tmpdir(), 'apeiron-local-access-'));
  const crt = join(directory, 'fixture.crt');
  await exec('openssl', ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '1',
    '-subj', '/CN=local-access-test', '-addext', 'basicConstraints=critical,CA:TRUE', '-keyout', join(directory, 'fixture.key'), '-out', crt]);
  const ca = publicCa(await readFile(crt, 'utf8'));
  artifacts = { info: { domain: 'team.internal', entryIp: '127.0.0.1', local: true, notes: [], ca: { path: crt, fingerprint: ca.fingerprint, expiresAt: ca.expiresAt } },
    ca: ca.pem, hosts: '# generated\n127.0.0.1 apeiron.team.internal\n127.0.0.1 iam.team.internal\n127.0.0.1 task.team.internal\n' };
});
afterAll(async () => { await rm(directory, { recursive: true, force: true }); });

test('hosts merge preserves existing aliases and other deployments, and remains idempotent', () => {
  const before = '## system\n127.0.0.1 localhost localalias # do not remove\n::1 localhost\n192.0.2.2 unrelated.internal\n# BEGIN APEIRON other.internal\n127.0.0.1 apeiron.other.internal\n# END APEIRON other.internal\n';
  const first = mergeHosts(before, artifacts);
  expect(first).toContain(before);
  expect(first).toContain('# BEGIN APEIRON team.internal\n');
  expect(mergeHosts(first, artifacts)).toBe(first);
  expect(first.match(/127.0.0.1 apeiron.team.internal/g)).toHaveLength(1);
  const older = first.replace('127.0.0.1 task.team.internal', '127.0.0.1 removed.team.internal');
  expect(mergeHosts(older, artifacts)).toBe(first);
  expect(() => mergeHosts('192.0.2.7 custom APEIRON.TEAM.INTERNAL # conflict\n', artifacts)).toThrow('其他 IP');
  expect(() => mergeHosts('::1 apeiron.team.internal\n', artifacts)).toThrow('其他 IP');
  for (const malformed of ['# BEGIN APEIRON team.internal\n', '# END APEIRON team.internal\n', first + first,
    '# BEGIN APEIRON team.internal\n192.0.2.9 unrelated.internal\n# END APEIRON team.internal\n']) {
    expect(() => mergeHosts(malformed, artifacts)).toThrow();
  }
});

test('privileged installation only accepts the generated hostnames and exact public CA', async () => {
  for (const hosts of ['127.0.0.1 unrelated.internal\n', artifacts.hosts + '127.0.0.1 foo.team.internal extra\n', artifacts.hosts + '192.0.2.1 bad.team.internal\n']) {
    expect(() => mergeHosts('', { ...artifacts, hosts })).toThrow();
  }
  expect(() => mergeHosts('', { ...artifacts, ca: artifacts.ca + '\n-----BEGIN PRIVATE KEY-----\n' })).toThrow();
  expect(() => mergeHosts('', { ...artifacts, info: { ...artifacts.info, ca: { ...artifacts.info.ca!, fingerprint: 'wrong' } } })).toThrow('指纹');
  expect(() => mergeHosts('', { ...artifacts, info: { ...artifacts.info, domain: 'foo;touch /tmp/x' } })).toThrow();
  const script = macAccessScript('# quotes: " \' \\ $(touch /tmp/no) `whoami`\n', artifacts);
  const path = join(directory, 'syntax.sh');
  await writeFile(path, script);
  await exec('/bin/sh', ['-n', path]);
  expect(script).toContain('add-trusted-cert -d -r trustRoot -p ssl');
  expect(script).not.toContain('PRIVATE KEY');
  if (process.platform === 'darwin') {
    const apple = join(directory, 'authorization.applescript');
    await writeFile(apple, macAuthorizationScript(script));
    // Compile the actual authorization syntax; never execute a system prompt in tests.
    await exec('/usr/bin/osacompile', ['-o', join(directory, 'authorization.scpt'), apple]);
  }
});

test('privileged script backs up hosts and preserves permissions; CA failure and concurrent edits never overwrite hosts', async () => {
  for (const mode of ['ok', 'ca-fails', 'hosts-changed'] as const) {
    const dir = await mkdtemp(join(directory, 'script-'));
    const hosts = join(dir, 'hosts');
    const before = '127.0.0.1 localhost\n# $(touch SHOULD_NOT_EXECUTE)\n';
    await writeFile(hosts, before, { mode: 0o644 });
    const security = join(dir, 'security');
    const escaped = hosts.replaceAll("'", "'\\''");
    await writeFile(security, '#!/bin/sh\n' + (mode === 'ca-fails' ? 'exit 1\n' : mode === 'hosts-changed' ? `printf '%s\\n' '# concurrent edit' >> '${escaped}'\n` : 'exit 0\n'), { mode: 0o755 });
    // Exercise the exact root transaction with only system locations/side effects
    // redirected into this test's temporary directory; no sudo or system keychain.
    const script = macAccessScript(before, artifacts).replaceAll('/private/etc', dir)
      .replace('/usr/bin/security', `'${security}'`).replace('/usr/bin/dscacheutil -flushcache', ':').replace('/usr/bin/killall -HUP mDNSResponder', ':');
    const path = join(dir, 'install.sh'); await writeFile(path, script);
    if (mode === 'ok') {
      const { stdout } = await exec('/bin/sh', [path]);
      const backup = stdout.trim().split('APEIRON_BACKUP=')[1]!;
      expect(await readFile(backup, 'utf8')).toBe(before);
      expect(await readFile(hosts, 'utf8')).toBe(mergeHosts(before, artifacts));
      expect((await stat(hosts)).mode & 0o777).toBe(0o644);
    } else {
      await expect(exec('/bin/sh', [path])).rejects.toThrow(mode === 'ca-fails' ? 'APEIRON_CA_FAILED' : 'APEIRON_HOSTS_CHANGED');
      expect(await readFile(hosts, 'utf8')).toBe(before + (mode === 'hosts-changed' ? '# concurrent edit\n' : ''));
    }
  }
});

test('installer reserves the operation before async work, handles cancellation, retry and HTTPS failure independently of deployment', async () => {
  let release!: () => void;
  let authorizationCalls = 0;
  let cancelled = true, passed = true;
  const installer = new LocalAccessInstaller({
    capability: async () => { await new Promise<void>(resolve => { release = resolve; }); return { available: true, host: 'fixture', reason: '' }; },
    readHosts: async () => '127.0.0.1 localhost\n',
    authorize: async () => { authorizationCalls++; if (cancelled) throw new Error('User canceled. (-128)'); return 'APEIRON_BACKUP=/private/etc/apeiron-access.abc123/hosts.before'; },
    verify: async () => ['apeiron', 'iam'].map(name => ({ host: `${name}.team.internal`, passed })),
  });
  const start = installer.start(artifacts);
  expect(installer.active).toBe(true);
  await expect(installer.start(artifacts)).rejects.toThrow('正在进行');
  release(); await start; await installer.wait();
  expect(installer.snapshot.phase).toBe('cancelled'); expect(authorizationCalls).toBe(1);
  cancelled = false;
  const retry = installer.start(artifacts); release(); await retry; await installer.wait();
  expect(installer.snapshot.phase).toBe('succeeded'); expect(installer.snapshot.checks).toHaveLength(2);
  passed = false;
  const failing = installer.start(artifacts); release(); await failing; await installer.wait();
  expect(installer.snapshot.phase).toBe('failed'); expect(installer.snapshot.message).toContain('配置已安装');
  expect(installer.snapshot.backup).toContain('hosts.before');
});

test('web install rejects cross-origin, missing tokens, parameters and incomplete deployments without prompting', async () => {
  let calls = 0;
  const installer = new LocalAccessInstaller({ capability: async () => ({ available: true, host: 'fixture', reason: '' }),
    readHosts: async () => '', authorize: async () => { calls++; return ''; }, verify: async () => [] });
  const server = await startInitServer({ path: join(directory, 'config.json'), localAccess: installer });
  try {
    const post = (url: string, origin: string, body: unknown) => fetch(url, { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    expect((await post(server.url + 'api/access/install', 'https://untrusted.example', {})).status).toBe(403);
    expect((await post(server.origin + '/setup/wrong/api/access/install', server.origin, {})).status).toBe(404);
    expect((await post(server.url + 'api/access/install', server.origin, {})).status).toBe(409);
    expect((await post(server.url + 'api/access/install', server.origin, { path: '/tmp/ca.crt' })).status).toBe(409);
    expect(calls).toBe(0);
    const state = await fetch(server.url + 'api/access').then(response => response.json());
    expect(state.status.phase).toBe('idle'); expect(state.capability.host).toBe('fixture');
  } finally { await server.stop(); }
});

test('completed deployment binds access installation to its artifacts and blocks redeployment/finish while authorization is pending', async () => {
  const dir = await mkdtemp(join(directory, 'api-'));
  const setup = await deploymentFixture(dir);
  await freshInstallFixture(dir, setup, 0, 'example.internal');
  const oldPath = process.env.PATH;
  process.env.PATH = setup.bin + ':' + oldPath;
  let release!: () => void;
  let calls = 0;
  const authorization = new Promise<void>(resolve => { release = resolve; });
  const installer = new LocalAccessInstaller({ capability: async () => ({ available: true, host: 'fixture', reason: '' }),
    readHosts: async () => '127.0.0.1 localhost\n', authorize: async script => {
      calls++; expect(script).toContain('apeiron.example.internal'); expect(script).not.toContain('attacker.internal');
      await authorization; return 'APEIRON_BACKUP=/private/etc/apeiron-access.fixture/hosts.before';
    }, verify: async () => [{ host: 'apeiron.example.internal', passed: true }, { host: 'iam.example.internal', passed: true }] });
  const server = await startInitServer({ path: join(dir, 'config.json'), localAccess: installer, resources: async () => setup.root });
  const post = (endpoint: string, body: unknown) => fetch(server.url + 'api/' + endpoint, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  try {
    const response = await post('deploy', { revision: null, slug: 'example', apps: requiredApps, deployment: { offline: false,
      installation: { ...installationDefaults(), topology: 'single-k3d', httpPort: 54320, httpsPort: 54321, domain: 'example.internal', entryIp: '127.0.0.1' } } });
    expect(response.status).toBe(202);
    for (let i = 0; i < 400 && server.result.phase !== 'succeeded' && server.result.phase !== 'failed'; i++) await Bun.sleep(10);
    expect(server.result.phase).toBe('succeeded'); expect(server.result.access?.domain).toBe('example.internal');
    expect((await post('access/install', { domain: 'attacker.internal', ca: 'arbitrary' })).status).toBe(400);
    expect(calls).toBe(0);
    expect((await post('access/install', {})).status).toBe(202);
    expect((await post('access/install', {})).status).toBe(409);
    expect((await post('finish', {})).status).toBe(409);
    expect((await post('deploy', {})).status).toBe(409);
    expect((await post('config', {})).status).toBe(409);
    const status = await fetch(server.url + 'api/access').then(response => response.json());
    expect(status.status.phase).toBe('installing'); expect(calls).toBe(1);
    expect(server.result.phase).toBe('succeeded');
    release(); await installer.wait();
    expect(installer.snapshot.phase).toBe('succeeded');
  } finally { release(); await server.stop(); process.env.PATH = oldPath; }
}, 10_000);
