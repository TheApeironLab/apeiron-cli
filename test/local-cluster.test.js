import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { localInstallation as installationDefaults } from './support';
import { startInitServer } from './rust-server';
import { deploymentFixture, freshInstallFixture, requiredApps } from './fixtures';
const cleanup = [];
afterEach(async () => { for (const fn of cleanup.splice(0).reverse())
    await fn(); });
async function fixture() {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-port-check-'));
    cleanup.push(() => rm(dir, { recursive: true, force: true }));
    const path = join(dir, 'config.json');
    const installation = { ...installationDefaults(), topology: 'single-k3d', domain: 'team.example.internal', entryIp: '127.0.0.1', httpPort: 54320, httpsPort: 54321 };
    const hash = value => createHash('sha256').update(value).digest('hex').slice(0, 12);
    const workDir = join(dir, 'deployments', 'k3d-' + hash(path + '|' + installation.domain));
    return { dir, path, installation, identity: { workDir } };
}
test('occupied port is rejected before saving, downloading or creating a deployment', async () => {
    const f = await fixture();
    const setup = await deploymentFixture(f.dir);
    const pathBefore = process.env.PATH;
    process.env.PATH = setup.bin + ':' + pathBefore;
    cleanup.push(async () => { process.env.PATH = pathBefore; });
    const occupied = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: () => new Response('other service') });
    cleanup.push(async () => { await occupied.stop(true); });
    const server = await startInitServer({ path: f.path, env: { APEIRON_CHENTU_ROOT: setup.root } });
    cleanup.push(server.stop);
    const response = await fetch(server.url + 'api/deploy', { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision: null, slug: 'team', apps: requiredApps, deployment: { offline: false, installation: { ...f.installation, httpPort: occupied.port, httpsPort: occupied.port === 54321 ? 54323 : 54321 } } }) });
    expect(response.status).toBe(409);
    expect((await response.json()).error).toContain(`${occupied.port} 端口已被占用`);
    expect(server.result.phase).toBe('idle');
    expect(server.result.log).toBeUndefined();
    expect(await Bun.file(f.path).exists()).toBe(false);
    expect(await Bun.file(setup.calls).exists()).toBe(false);
});
test('owned cluster enters redeployment immediately, preserving the saved identity', async () => {
    const f = await fixture();
    const setup = await deploymentFixture(f.dir);
    await freshInstallFixture(f.dir, setup, 0, f.installation.domain);
    const pathBefore = process.env.PATH;
    process.env.PATH = setup.bin + ':' + pathBefore;
    cleanup.push(async () => { process.env.PATH = pathBefore; });
    const marker = await readFile(join(f.identity.workDir, 'installation.json'), 'utf8');
    const server = await startInitServer({ path: f.path, env: { APEIRON_CHENTU_ROOT: setup.root } });
    cleanup.push(server.stop);
    const response = await fetch(server.url + 'api/deploy', { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision: null, slug: 'team', apps: requiredApps, deployment: { offline: false, installation: f.installation } }) });
    expect(response.status).toBe(202);
    for (let count = 0; count < 100 && !server.result.events.some(event => event.includes('复用集群重新部署')); count++)
        await Bun.sleep(10);
    expect(server.result.events).toContain('[INFO] 已识别本次安装的 K3d 集群，将复用集群重新部署，保留数据和凭据。');
    expect(await readFile(join(f.identity.workDir, 'installation.json'), 'utf8')).toBe(marker);
});
