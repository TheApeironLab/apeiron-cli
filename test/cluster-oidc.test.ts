import { expect, test } from 'bun:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Deployment } from '../src/init/deploy';
import { validateConfig } from '../src/init/config';
import { installationDefaults } from '../src/init/installation';
import { deploymentFixture, freshInstallFixture, requiredApps } from './fixtures';

test('fresh installation requires OIDC bundle support before any cluster operation', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-oidc-gate-'));
  const setup = await deploymentFixture(dir);
  await freshInstallFixture(dir, setup, 0, 'example.internal');
  const catalogPath = join(setup.root, 'setup/install.json');
  const catalog = await Bun.file(catalogPath).json();
  for (const target of Object.values(catalog.targets) as any[]) delete target.clusterOidc;
  await writeFile(catalogPath, JSON.stringify(catalog));
  const deployment = new Deployment(join(dir, 'config.json'), undefined, async () => setup.root);
  try {
    await deployment.start(async () => validateConfig({ slug: 'fixture', apps: requiredApps, deployment: {
      offline: false, installation: { ...installationDefaults(), topology: 'single-k3d', domain: 'example.internal', entryIp: '127.0.0.1' },
    } }));
    while (deployment.active) await Bun.sleep(10);
    expect(deployment.snapshot.phase).toBe('failed');
    expect(deployment.snapshot.message).toContain('clusterOidc');
    expect(await Bun.file(setup.calls).exists()).toBe(false);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('OIDC failure prevents success and credentials; retry finishes OIDC after Helmfile', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-oidc-finish-'));
  const setup = await deploymentFixture(dir);
  await freshInstallFixture(dir, setup, 0, 'example.internal');
  const catalogPath = join(setup.root, 'setup/install.json');
  const catalog = await Bun.file(catalogPath).json();
  const change = async (exit: number) => {
    for (const target of Object.values(catalog.targets) as any[]) {
      target.environment.fixtureOidcExit = exit;
      target.environment.fixtureDelay = 10;
    }
    await writeFile(catalogPath, JSON.stringify(catalog));
  };
  await change(17);
  const oldPath = process.env.PATH; process.env.PATH = setup.bin + ':' + oldPath;
  const deployment = new Deployment(join(dir, 'config.json'), undefined, async () => setup.root);
  const config = validateConfig({ slug: 'fixture', apps: requiredApps, deployment: {
    offline: false, installation: { ...installationDefaults(), topology: 'single-k3d', domain: 'example.internal', entryIp: '127.0.0.1' },
  } });
  try {
    await deployment.start(async () => config);
    while (deployment.active) await Bun.sleep(10);
    expect(deployment.snapshot.phase).toBe('failed');
    expect(deployment.snapshot.message).toContain('SSO');
    expect(deployment.snapshot.access).toBeUndefined();
    expect(deployment.snapshot.exitCode).not.toBe(0);
    await expect(deployment.initialAdmin(new AbortController().signal)).rejects.toThrow('部署成功后');
    await change(0);
    await deployment.start(async () => config);
    while (deployment.active) await Bun.sleep(10);
    expect(deployment.snapshot.phase).toBe('succeeded');
    const calls = (await readFile(setup.calls, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
    expect(calls.map(call => call.args[0])).toEqual(['prepare', 'sync', 'configure-oidc', 'prepare', 'sync', 'configure-oidc']);
    expect(calls.every(call => call.values.releases['cluster-access'].enabled)).toBe(true);
  } finally { process.env.PATH = oldPath; await rm(dir, { recursive: true, force: true }); }
}, 10_000);

test('installation can stop during OIDC finalization and retry without reporting success', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-oidc-stop-'));
  const setup = await deploymentFixture(dir);
  await freshInstallFixture(dir, setup, 0, 'example.internal');
  const path = join(setup.root, 'setup/install.json');
  const catalog = await Bun.file(path).json();
  const update = async (delay: number) => {
    for (const target of Object.values(catalog.targets) as any[]) {
      target.environment.fixtureDelay = 10;
      target.environment.fixtureOidcDelay = delay;
    }
    await writeFile(path, JSON.stringify(catalog));
  };
  await update(30_000);
  const oldPath = process.env.PATH; process.env.PATH = setup.bin + ':' + oldPath;
  const deployment = new Deployment(join(dir, 'config.json'), undefined, async () => setup.root);
  const config = validateConfig({ slug: 'fixture', apps: requiredApps, deployment: { offline: false,
    installation: { ...installationDefaults(), topology: 'single-k3d', domain: 'example.internal', entryIp: '127.0.0.1' },
  } });
  try {
    await deployment.start(async () => config);
    for (let i = 0; i < 400 && !deployment.snapshot.message.includes('正在配置集群 SSO'); i++) await Bun.sleep(10);
    expect(deployment.snapshot.message).toContain('正在配置集群 SSO');
    expect(deployment.active).toBe(true);
    await deployment.stop();
    expect(deployment.snapshot.phase).toBe('cancelled');
    expect(deployment.snapshot.access).toBeUndefined();
    await update(0);
    await deployment.start(async () => config);
    while (deployment.active) await Bun.sleep(10);
    expect(deployment.snapshot.phase).toBe('succeeded');
  } finally { if (deployment.active) await deployment.stop(); process.env.PATH = oldPath; await rm(dir, { recursive: true, force: true }); }
}, 10_000);
