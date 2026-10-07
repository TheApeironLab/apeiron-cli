import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { checkLocalCluster, localCluster } from '../src/init/local-cluster';
import { startInitServer } from '../src/init/server';
import { installationDefaults } from '../src/init/installation';
import { deploymentFixture, freshInstallFixture, requiredApps } from './fixtures';

const cleanup: (() => Promise<unknown>)[] = [];
afterEach(async () => { for (const fn of cleanup.splice(0).reverse()) await fn(); });
async function fixture() {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-port-check-'));
  cleanup.push(() => rm(dir, { recursive: true, force: true }));
  const path = join(dir, 'config.json');
  const installation = { ...installationDefaults(), topology: 'single-k3d' as const, domain: 'team.example.internal', entryIp: '127.0.0.1', httpPort: 54320, httpsPort: 54321 };
  const identity = localCluster(path, installation.domain);
  const existing = { cluster: identity.cluster, running: true, bindings: {
    '54320/tcp': [{ HostIp: '127.0.0.1', HostPort: '54320' }], '443/tcp': [{ HostIp: '127.0.0.1', HostPort: '54321' }],
  } };
  let checkedPorts: number[] = [];
  const checks = { inspect: async () => existing, checkPort: async (port: number) => { checkedPorts.push(port); } };
  const signal = new AbortController().signal;
  const own = async () => {
    await mkdir(identity.workDir, { recursive: true });
    await writeFile(join(identity.workDir, 'installation.json'), JSON.stringify({ cluster: identity.cluster, domain: installation.domain }));
  };
  return { dir, path, installation, identity, existing, checks, checkedPorts, signal, own };
}

test('owned cluster redeploys on occupied ports; unrelated or changed cluster is never adopted', async () => {
  const f = await fixture();
  await expect(checkLocalCluster(f.identity, f.installation, f.signal, f.checks)).rejects.toThrow('不属于本次安装');
  await f.own();
  expect((await checkLocalCluster(f.identity, f.installation, f.signal, f.checks)).mode).toBe('redeploy');
  expect(f.checkedPorts).toEqual([]);
  await expect(checkLocalCluster(f.identity, { ...f.installation, httpsPort: 54323 }, f.signal, f.checks)).rejects.toThrow('端口与当前配置不同');
  await expect(checkLocalCluster(f.identity, { ...f.installation, domain: 'other.example.internal' }, f.signal, f.checks)).rejects.toThrow('不属于本次安装');
  f.existing.running = false;
  await expect(checkLocalCluster(f.identity, f.installation, f.signal, f.checks)).rejects.toThrow('已停止');
});

test('fresh install checks both ports, and swapped container bindings cannot pass as a redeploy', async () => {
  const f = await fixture();
  expect((await checkLocalCluster(f.identity, f.installation, f.signal, { ...f.checks, inspect: async () => null })).mode).toBe('install');
  expect(f.checkedPorts).toEqual([54320, 54321]);
  await f.own();
  f.existing.bindings['54320/tcp'][0]!.HostPort = '54321';
  f.existing.bindings['443/tcp'][0]!.HostPort = '54320';
  await expect(checkLocalCluster(f.identity, f.installation, f.signal, f.checks)).rejects.toThrow('端口与当前配置不同');
});

test('occupied port is rejected before saving, downloading or creating a deployment', async () => {
  const f = await fixture();
  const setup = await deploymentFixture(f.dir);
  const pathBefore = process.env.PATH;
  process.env.PATH = setup.bin + ':' + pathBefore;
  cleanup.push(async () => { process.env.PATH = pathBefore; });
  const occupied = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: () => new Response('other service') });
  cleanup.push(async () => { await occupied.stop(true); });
  let downloads = 0;
  const server = await startInitServer({ path: f.path, resources: async () => { downloads++; return setup.root; } });
  cleanup.push(server.stop);
  const response = await fetch(server.url + 'api/deploy', { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' },
    body: JSON.stringify({ revision: null, slug: 'team', apps: requiredApps, deployment: { offline: false, installation: { ...f.installation, httpPort: occupied.port, httpsPort: occupied.port === 54321 ? 54323 : 54321 } } }) });
  expect(response.status).toBe(409);
  expect((await response.json()).error).toContain(`${occupied.port} 端口已被占用（其他集群或服务）`);
  expect(server.result.phase).toBe('idle');
  expect(server.result.log).toBeUndefined();
  expect(await Bun.file(f.path).exists()).toBe(false);
  expect(downloads).toBe(0);
});

test('owned cluster enters redeployment immediately, preserving the saved identity', async () => {
  const f = await fixture();
  const setup = await deploymentFixture(f.dir);
  await freshInstallFixture(f.dir, setup, 0, f.installation.domain);
  const pathBefore = process.env.PATH;
  process.env.PATH = setup.bin + ':' + pathBefore;
  cleanup.push(async () => { process.env.PATH = pathBefore; });
  const marker = await readFile(join(f.identity.workDir, 'installation.json'), 'utf8');
  const server = await startInitServer({ path: f.path, resources: async () => setup.root });
  cleanup.push(server.stop);
  const response = await fetch(server.url + 'api/deploy', { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' },
    body: JSON.stringify({ revision: null, slug: 'team', apps: requiredApps, deployment: { offline: false, installation: f.installation } }) });
  expect(response.status).toBe(202);
  for (let count = 0; count < 100 && !server.result.events.some(event => event.includes('复用集群重新部署')); count++) await Bun.sleep(10);
  expect(server.result.events).toContain('[INFO] 已识别本次安装的 K3d 集群，将复用集群重新部署，保留数据和凭据。');
  expect(await readFile(join(f.identity.workDir, 'installation.json'), 'utf8')).toBe(marker);
});
