import { expect, test } from 'bun:test';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, symlink } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { checkDns } from '../src/init/dns';
import { collectAccess, publicCa } from '../src/init/access';
import { checkLocalPort } from '../src/init/bootstrap';
import { installationDefaults, validateInstallation } from '../src/init/installation';
import { startInitServer } from '../src/init/server';
import { deploymentFixture } from './fixtures';

test('DNS requires every returned address to match, and verifies wildcard independently of app records', async () => {
  const seen: string[] = [];
  const input = { domain: 'team.apeironlab.internal', entryIp: '192.0.2.10', local: false };
  const good = await checkDns(input, { lookup: async host => { seen.push(host); return [{ address: input.entryIp }]; } });
  expect(good.passed).toBe(true);
  expect(seen).toHaveLength(3);
  expect(seen[2]).toMatch(/^apeiron-check-[a-f0-9]+\.team\.apeironlab\.internal$/);
  const wildcard = await checkDns(input, { lookup: async host => {
    if (host.startsWith('apeiron-check-')) throw new Error('NXDOMAIN');
    return [{ address: input.entryIp }];
  } });
  expect(wildcard.passed).toBe(false);
  expect(wildcard.checks[2]!.status).toBe('unresolved');
  expect((await checkDns(input, { lookup: async () => [{ address: input.entryIp }, { address: '192.0.2.11' }] })).passed).toBe(false);
  expect((await checkDns(input, { lookup: async () => [], timeoutMs: 10 })).passed).toBe(false);
});

test('local hosts checks omit wildcard; DNS timeouts and cancellation are bounded', async () => {
  const input = { domain: 'team.apeironlab.internal', entryIp: '127.0.0.1', local: true };
  const result = await checkDns(input, { lookup: async () => [{ address: input.entryIp }] });
  expect(result.checks).toHaveLength(2); expect(result.wildcard).toBe(false); expect(result.passed).toBe(true);
  const hung = await checkDns(input, { lookup: () => new Promise(() => {}), timeoutMs: 10 });
  expect(hung.checks.every(check => check.status === 'timeout')).toBe(true);
  const controller = new AbortController(); controller.abort();
  const stopped = await checkDns(input, { lookup: async () => { throw new Error('should not run'); }, signal: controller.signal });
  expect(stopped.passed).toBe(false);
  for (const domain of ['bad;command', '127.0.0.1', 'wrong..internal', 'UPPER.internal', 'x'.repeat(64) + '.internal']) {
    await expect(checkDns({ ...input, domain })).rejects.toThrow();
  }
  expect(() => validateInstallation({ ...installationDefaults(), topology: 'single-k3s', domain: input.domain, entryIp: '127.0.0.1' })).toThrow();
  expect(() => validateInstallation({ ...installationDefaults(), topology: 'single-k3d', domain: input.domain, entryIp: '192.0.2.10' })).toThrow();
});

test('DNS and download routes keep origin and token protections and never accept arbitrary paths', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-dns-api-'));
  const server = await startInitServer({ path: join(directory, 'config.json') });
  try {
    const post = (origin: string, body: unknown) => fetch(server.url + 'api/dns', { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    expect((await post('https://untrusted.example', {})).status).toBe(403);
    expect((await post(server.origin, { domain: 'bad/host', entryIp: '127.0.0.1', local: true })).status).toBe(400);
    for (const file of ['ca.crt', 'hosts.txt', 'ca.crt?path=/etc/passwd', 'ca.key']) expect((await fetch(server.url + 'api/' + file)).status).toBe(404);
    const config = await fetch(server.url + 'api/config').then(response => response.json());
    expect(typeof config.host.name).toBe('string');
    expect(Array.isArray(config.host.addresses)).toBe(true);
  } finally { await server.stop(); await rm(directory, { recursive: true, force: true }); }
});

test('certificate export accepts only a valid public CA; private keys, leaf certificates and symlinks are not served', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-ca-'));
  try {
    const crt = join(directory, 'certificate.crt'), key = join(directory, 'private.key');
    await promisify(execFile)('openssl', ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '1',
      '-subj', '/CN=fixture only', '-addext', 'basicConstraints=critical,CA:TRUE', '-keyout', key, '-out', crt]);
    const pem = await readFile(crt, 'utf8'), privateKey = await readFile(key, 'utf8');
    expect(publicCa(pem).fingerprint).toMatch(/^(?:[A-F0-9]{2}:){31}[A-F0-9]{2}$/);
    expect(publicCa(pem).pem).not.toContain('PRIVATE KEY');
    for (const invalid of [privateKey, pem + privateKey, pem + pem, 'invalid']) expect(() => publicCa(invalid)).toThrow();
    await promisify(execFile)('openssl', ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '1',
      '-subj', '/CN=leaf only', '-addext', 'basicConstraints=critical,CA:FALSE', '-keyout', key, '-out', crt]);
    const leaf = await readFile(crt, 'utf8');
    expect(() => publicCa(leaf)).toThrow();
    const fixture = await deploymentFixture(directory);
    await symlink(key, join(fixture.workDir, 'state/chentu-ca.crt'));
    const result = await collectAccess({ ...fixture.target, installation: { ...installationDefaults(), topology: 'single-k3d', domain: 'example.internal', entryIp: '127.0.0.1' } }, directory, new AbortController().signal);
    expect(result.ca).toBeUndefined(); expect(result.info.ca).toBeUndefined();
    expect(JSON.stringify(result)).not.toContain(privateKey);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('local port preflight reports an occupied entry port', async () => {
  const server = createServer();
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address() as { port: number };
  try { await expect(checkLocalPort(address.port)).rejects.toThrow('已被占用'); }
  finally { await new Promise<void>(resolve => server.close(() => resolve())); }
  await checkLocalPort(address.port);
});
