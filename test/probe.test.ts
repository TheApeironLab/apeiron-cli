import { expect, test } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { EnvironmentProbe, linuxCpuModel, readMachine, type MachineInfo } from '../src/init/probe';
import { startInitServer } from '../src/init/server';

const machine: MachineInfo = {
  os: { name: 'Ubuntu', version: '22.04', kernel: 'test-kernel' },
  hardware: { architecture: 'amd64', runtimeArchitecture: 'amd64', cpu: 'Test CPU', cores: 8, memoryGiB: 16 },
};
const host = async () => machine;

test('Linux ARM falls back to lscpu and retains both Spark core models', async () => {
  const result = await linuxCpuModel(['unknown', 'unknown'], async (file, args) => {
    expect(file).toBe('lscpu'); expect(args).toEqual(['--json']);
    return JSON.stringify({ lscpu: [
      { field: 'Vendor ID:', data: 'ARM' },
      { field: 'Model name:', data: 'Cortex-X925' },
      { field: 'Model name:', data: 'Cortex-A725' },
      { field: 'Model name:', data: 'Cortex-X925' },
    ] });
  });
  expect(result).toBe('Cortex-X925 + Cortex-A725');
});

test('CPU fallback accepts nested lscpu output and never guesses missing names', async () => {
  expect(await linuxCpuModel(['unknown'], async () => JSON.stringify({ lscpu: [
    { field: 'Vendor ID:', data: 'ARM', children: [{ field: 'Model name:', data: 'Cortex-X925' }] },
  ] }))).toBe('Cortex-X925');
  for (const response of ['', 'invalid json', '{}', '{"lscpu":[{"field":"Model name:","data":"unknown"}]}']) {
    expect(await linuxCpuModel(['unknown'], async () => response)).toBe('');
  }
  expect(await linuxCpuModel(['known', 'unknown'], async () => { throw new Error('lscpu unavailable'); })).toBe('known');
  let calls = 0;
  expect(await linuxCpuModel(['AMD processor', 'AMD processor'], async () => { calls++; return ''; })).toBe('AMD processor');
  expect(calls).toBe(0);
});

test('offline probe reads the local host without making any network request', async () => {
  let calls = 0;
  const probe = new EnvironmentProbe(async () => { calls++; throw new Error('must not call'); }, readMachine);
  const result = await probe.run(true);
  expect(calls).toBe(0);
  expect(result.network).toEqual({ status: 'skipped', checks: [] });
  expect(result.machine.os.name.length).toBeGreaterThan(0);
  expect(result.machine.hardware.architecture.length).toBeGreaterThan(0);
  expect(result.machine.hardware.cores).toBeGreaterThan(0);
  expect(result.machine.hardware.memoryGiB).toBeGreaterThan(0);
});

test('probe distinguishes Google timeout, DNS failure and a reachable download host; never sends configuration', async () => {
  const calls: Array<{ url: string; options: RequestInit }> = [];
  const probe = new EnvironmentProbe(async (url, options) => {
    calls.push({ url, options });
    if (url === 'https://www.google.com/') throw { cause: { code: 'ETIMEDOUT' } };
    if (url === 'https://api.github.com/') throw Object.assign(new Error('sensitive proxy diagnostic'), { code: 'ENOTFOUND' });
    return new Response(null, { status: url.includes('release-assets.') ? 404 : 200 });
  }, host);
  const result = await probe.run(false);
  expect(result.machine).toEqual(machine);
  expect(result.network.status).toBe('limited');
  expect(result.network.checks.map(check => check.status)).toEqual(['reachable', 'timeout', 'dns-error', 'reachable']);
  expect(result.network.checks[1]?.host).toBe('www.google.com');
  expect(result.network.checks[3]?.httpStatus).toBe(404);
  expect(JSON.stringify(result)).not.toContain('sensitive');
  for (const call of calls) {
    expect(call.options.method).toBe('HEAD');
    expect(call.options.body).toBeUndefined();
    expect(call.options.redirect).toBe('manual');
    expect([...new Headers(call.options.headers).keys()]).toEqual(['user-agent']);
  }
  await probe.run(false);
  expect(calls.length).toBe(4); // A second page within 30 seconds reuses the result.
  await probe.run(false, true);
  expect(calls.length).toBe(8); // Explicit recheck bypasses the cache.
});

test('unresponsive network probes time out instead of blocking setup', async () => {
  const probe = new EnvironmentProbe((_url, options) => new Promise((_resolve, reject) => {
    options.signal!.addEventListener('abort', () => reject(new Error('timeout')), { once: true });
  }), host, 20);
  const result = await probe.run(false);
  expect(result.network.status).toBe('unreachable');
  expect(result.network.checks.map(check => check.status)).toEqual(['timeout', 'timeout', 'timeout', 'timeout']);
});

test('offline switch aborts pending external checks and drops the online cache', async () => {
  let calls = 0;
  let cancelled = 0;
  const probe = new EnvironmentProbe((_url, options) => new Promise((_resolve, reject) => {
    calls++;
    options.signal!.addEventListener('abort', () => { cancelled++; reject(new Error('aborted')); }, { once: true });
  }), host);
  const first = probe.run(false);
  const second = probe.run(false, true);
  expect(calls).toBe(4); // Concurrent clients share the bounded scan.
  const offline = await probe.run(true);
  expect(offline.network.status).toBe('skipped');
  expect((await first).network.status).toBe('cancelled');
  expect((await second).network.status).toBe('cancelled');
  expect(cancelled).toBe(4);
  const next = probe.run(false);
  expect(calls).toBe(8);
  probe.cancel();
  expect((await next).network.status).toBe('cancelled');
});

test('HTTP restrictions and TLS errors are not reported as working download access', async () => {
  const probe = new EnvironmentProbe(async url => {
    if (url.includes('api.github')) throw { cause: { code: 'CERT_HAS_EXPIRED', privateValue: 'not-for-browser' } };
    return new Response(null, { status: 403 });
  }, host);
  const result = await probe.run(false);
  expect(result.network.status).toBe('unreachable');
  expect(result.network.checks.map(check => check.status)).toEqual(['http-error', 'http-error', 'tls-error', 'http-error']);
  expect(result.network.checks[0]?.httpStatus).toBe(403);
  expect(JSON.stringify(result)).not.toContain('not-for-browser');
});

test('probe endpoint requires same-origin POST and explicit mode, respects offline and never saves configuration', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-probe-api-'));
  const path = join(dir, 'config.json');
  let calls = 0;
  const probe = new EnvironmentProbe(async () => { calls++; return new Response(null, { status: 200 }); }, host);
  const server = await startInitServer({ path, probe });
  try {
    const post = (body: unknown, origin = server.origin) => fetch(server.url + 'api/probe', {
      method: 'POST', headers: { 'Content-Type': 'application/json', Origin: origin }, body: JSON.stringify(body),
    });
    expect((await fetch(server.url + 'api/probe')).status).toBe(404);
    expect((await post({ offline: false }, 'https://example.com')).status).toBe(403);
    for (const body of [{}, { offline: 'false' }, { offline: false, refresh: 'yes' }, null]) expect((await post(body)).status).toBe(400);
    const offline = await post({ offline: true }).then(response => response.json());
    expect(offline.network.status).toBe('skipped');
    expect(offline.machine.hardware.architecture).toBe('amd64');
    expect(calls).toBe(0);
    const online = await post({ offline: false }).then(response => response.json());
    expect(online.network.status).toBe('reachable');
    expect(calls).toBe(4);
    expect(await Bun.file(path).exists()).toBe(false);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});
