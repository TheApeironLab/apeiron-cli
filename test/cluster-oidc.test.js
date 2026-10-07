import { expect, test } from 'bun:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DeploymentClient, localInstallation as installationDefaults } from './support';
import { deploymentFixture, freshInstallFixture, requiredApps } from './fixtures';
test('fresh installation requires OIDC bundle support before any cluster operation', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-oidc-gate-'));
    const setup = await deploymentFixture(dir);
    await freshInstallFixture(dir, setup, 0, 'example.internal');
    const catalogPath = join(setup.root, 'setup/install.json');
    const catalog = await Bun.file(catalogPath).json();
    for (const target of Object.values(catalog.targets))
        delete target.clusterOidc;
    await writeFile(catalogPath, JSON.stringify(catalog));
    const deployment = new DeploymentClient(join(dir, 'config.json'), setup.root, { PATH: setup.bin + ':' + process.env.PATH });
    try {
        await deployment.start(({ slug: 'fixture', apps: requiredApps, deployment: {
                offline: false, installation: { ...installationDefaults(), topology: 'single-k3d', httpPort: 54320, httpsPort: 54321, domain: 'example.internal', entryIp: '127.0.0.1' },
            } }));
        while (deployment.active)
            await Bun.sleep(10);
        expect(deployment.snapshot.phase).toBe('failed');
        expect(deployment.snapshot.message).toContain('集群 SSO 初始化');
        expect(await Bun.file(setup.calls).exists()).toBe(false);
    }
    finally {
        await deployment.close();
        await rm(dir, { recursive: true, force: true });
    }
});
test('OIDC failure prevents success and credentials; retry finishes OIDC after Helmfile', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-oidc-finish-'));
    const setup = await deploymentFixture(dir);
    await freshInstallFixture(dir, setup, 0, 'example.internal');
    const catalogPath = join(setup.root, 'setup/install.json');
    const catalog = await Bun.file(catalogPath).json();
    const change = async (exit) => {
        for (const target of Object.values(catalog.targets)) {
            target.environment.fixtureOidcExit = exit;
            target.environment.fixtureDelay = 10;
        }
        await writeFile(catalogPath, JSON.stringify(catalog));
    };
    await change(17);
    const oldPath = process.env.PATH;
    process.env.PATH = setup.bin + ':' + oldPath;
    const deployment = new DeploymentClient(join(dir, 'config.json'), setup.root, { PATH: setup.bin + ':' + process.env.PATH });
    const config = ({ slug: 'fixture', apps: requiredApps, deployment: {
            offline: false, installation: { ...installationDefaults(), topology: 'single-k3d', httpPort: 54320, httpsPort: 54321, domain: 'example.internal', entryIp: '127.0.0.1' },
        } });
    try {
        await deployment.start(config);
        while (deployment.active)
            await Bun.sleep(10);
        expect(deployment.snapshot.phase).toBe('failed');
        expect(deployment.snapshot.message).toContain('SSO');
        expect(deployment.snapshot.access).toBeUndefined();
        expect(deployment.snapshot.exitCode).not.toBe(0);
        await expect(deployment.initialAdmin(new AbortController().signal)).rejects.toThrow('部署成功后');
        await change(0);
        await deployment.start(config);
        while (deployment.active)
            await Bun.sleep(10);
        expect(deployment.snapshot.phase).toBe('succeeded');
        const calls = (await readFile(setup.calls, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
        expect(calls.map(call => call.args[0])).toEqual(['prepare', 'sync', 'configure-oidc', 'prepare', 'sync', 'configure-oidc']);
        expect(calls.every(call => call.values.releases['cluster-access'].enabled)).toBe(true);
    }
    finally {
        await deployment.close();
        process.env.PATH = oldPath;
        await rm(dir, { recursive: true, force: true });
    }
}, 10_000);
test('installation can stop during OIDC finalization and retry without reporting success', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-oidc-stop-'));
    const setup = await deploymentFixture(dir);
    await freshInstallFixture(dir, setup, 0, 'example.internal');
    const path = join(setup.root, 'setup/install.json');
    const catalog = await Bun.file(path).json();
    const update = async (delay) => {
        for (const target of Object.values(catalog.targets)) {
            target.environment.fixtureDelay = 10;
            target.environment.fixtureOidcDelay = delay;
        }
        await writeFile(path, JSON.stringify(catalog));
    };
    await update(30_000);
    const oldPath = process.env.PATH;
    process.env.PATH = setup.bin + ':' + oldPath;
    const deployment = new DeploymentClient(join(dir, 'config.json'), setup.root, { PATH: setup.bin + ':' + process.env.PATH });
    const config = ({ slug: 'fixture', apps: requiredApps, deployment: { offline: false,
            installation: { ...installationDefaults(), topology: 'single-k3d', httpPort: 54320, httpsPort: 54321, domain: 'example.internal', entryIp: '127.0.0.1' },
        } });
    try {
        await deployment.start(config);
        for (let i = 0; i < 400 && !deployment.snapshot.message.includes('正在配置集群 SSO'); i++)
            await Bun.sleep(10);
        expect(deployment.snapshot.message).toContain('正在配置集群 SSO');
        expect(deployment.active).toBe(true);
        await deployment.stop();
        expect(deployment.snapshot.phase).toBe('cancelled');
        expect(deployment.snapshot.access).toBeUndefined();
        await update(0);
        await deployment.start(config);
        while (deployment.active)
            await Bun.sleep(10);
        expect(deployment.snapshot.phase).toBe('succeeded');
    }
    finally {
        await deployment.close();
        process.env.PATH = oldPath;
        await rm(dir, { recursive: true, force: true });
    }
}, 10_000);
