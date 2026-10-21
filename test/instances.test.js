import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startInitServer } from './rust-server';
import { deploymentFixture, requiredApps } from './fixtures';
import { localInstallation } from './support';
// Contract tests for the WP4 instance/preflight API the acceptance harness relies on.
const cleanups = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0).reverse())
    await cleanup(); });
async function fixture(configPath) {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-instances-test-'));
    cleanups.push(() => rm(dir, { recursive: true, force: true }));
    const setup = await deploymentFixture(dir);
    const home = join(dir, 'home');
    await mkdir(home, { recursive: true });
    // Stubs: no k3d cluster exists for any instance; deletions record their argv.
    await writeFile(join(setup.bin, 'docker'), '#!/bin/sh\nif [ "$1" = inspect ]; then exit 1; fi\nexit 0\n', { mode: 0o755 });
    await writeFile(join(setup.bin, 'k3d'), `#!/bin/sh\necho "$@" >> ${JSON.stringify(join(dir, 'k3d.log'))}\nexit 0\n`, { mode: 0o755 });
    const path = configPath ? configPath(home) : join(dir, 'config', 'config.json');
    const server = await startInitServer({ path, env: { HOME: home, PATH: setup.bin + ':' + process.env.PATH, APEIRON_CHENTU_ROOT: setup.root } });
    cleanups.push(server.stop);
    const post = (endpoint, body) => fetch(server.url + 'api/' + endpoint, {
        method: 'POST', headers: { 'Content-Type': 'application/json', Origin: server.origin }, body: JSON.stringify(body),
    });
    const config = () => fetch(server.url + 'api/config').then(r => r.json());
    const body = (revision = null) => ({ slug: 'example-team', apps: [...requiredApps], deployment: setup.target, revision });
    const installBody = async () => ({ ...body(), deployment: { ...(await config()).defaults, offline: false, bundleDir: '', installation: localInstallation() } });
    return { dir, home, path, setup, server, post, config, body, installBody };
}
test('instances: create registers and can select in one call; create is idempotent for the current instance', async () => {
    const { home, post, config, server } = await fixture();
    let snapshot = await config();
    expect(snapshot.instance).toBeNull();
    const listing = await fetch(server.url + 'api/instances').then(r => r.json());
    expect(listing.instances).toEqual([]);
    expect(listing.current).toBeNull();
    // 1. Native instances take 80/443; the response carries status fields.
    let response = await post('instances/create', { name: 'edge', topology: 'single-k3s' });
    expect(response.status).toBe(200);
    let created = await response.json();
    expect(created.existing).toBe(false);
    expect(created.selected).toBe(false);
    expect(created.instance).toMatchObject({ name: 'edge', topology: 'single-k3s', httpPort: 80, httpsPort: 443, phase: 'idle', k3d: null, domain: 'edge.apeironlab.internal' });
    expect(created.instance.config).toBe(join(home, '.apeiron/instances/edge/config.json'));
    // 2. select: true switches the wizard to the new K3d instance with allocated ports.
    response = await post('instances/create', { name: 'lab', topology: 'single-k3d', select: true });
    expect(response.status).toBe(200);
    created = await response.json();
    expect(created.selected).toBe(true);
    expect(created.path).toBe(join(home, '.apeiron/instances/lab/config.json'));
    expect(created.config.deployment.installation.topology).toBe('single-k3d');
    expect(created.instance.httpPort).toBeGreaterThanOrEqual(54320);
    expect(created.instance.httpsPort).toBe(created.instance.httpPort + 1);
    expect(created.deployment.phase).toBe('idle');
    snapshot = await config();
    expect(snapshot.path).toBe(created.path);
    expect(snapshot.instance.id).toBe(created.instance.id);
    expect(snapshot.config.deployment.installation.httpPort).toBe(created.instance.httpPort);
    // 3. Creating the current instance again by name is idempotent (start.sh + APEIRON_INSTANCE case).
    response = await post('instances/create', { name: 'lab', topology: 'single-k3d' });
    expect(response.status).toBe(200);
    const again = await response.json();
    expect(again.existing).toBe(true);
    expect(again.instance.id).toBe(created.instance.id);
    response = await post('instances/create', { name: 'lab', topology: 'multi-k3s' });
    expect(response.status).toBe(409);
    snapshot = await config();
    expect(snapshot.instance.topology).toBe('single-k3d');
    expect(snapshot.config.deployment.installation.topology).toBe('single-k3d');
    expect(snapshot.instance.httpPort).toBe(created.instance.httpPort);
    // 4. Other duplicates, bad names and unknown fields fail.
    expect((await post('instances/create', { name: 'edge', topology: 'single-k3d' })).status).toBe(409);
    expect((await post('instances/create', { name: 'Bad Name', topology: 'single-k3d' })).status).toBe(400);
    expect((await post('instances/create', { name: 'x', topology: 'single-k3d', extra: 1 })).status).toBe(400);
    expect((await post('instances/create', { name: 'x', topology: 'single-k3d', select: 'yes' })).status).toBe(400);
    const registry = JSON.parse(await readFile(join(home, '.apeiron/instances.json'), 'utf8'));
    expect(registry.instances.map(i => i.name)).toEqual(['edge', 'lab']);
});
test('instances: select and delete accept {id} or {name}; delete refuses the current instance and native topologies', async () => {
    const { dir, post, server } = await fixture();
    const edge = (await post('instances/create', { name: 'edge', topology: 'single-k3s' }).then(r => r.json())).instance;
    const lab = (await post('instances/create', { name: 'lab', topology: 'single-k3d' }).then(r => r.json())).instance;
    const spare = (await post('instances/create', { name: 'spare', topology: 'single-k3d' }).then(r => r.json())).instance;
    expect(spare.httpPort).toBe(lab.httpPort + 2);
    let response = await post('instances/select', { name: 'lab' });
    expect(response.status).toBe(200);
    expect((await response.json()).instance.id).toBe(lab.id);
    expect((await fetch(server.url + 'api/instances').then(r => r.json())).current).toBe(lab.id);
    expect((await post('instances/delete', { id: lab.id })).status).toBe(409);
    expect((await post('instances/delete', { name: 'edge' })).status).toBe(409);
    expect((await post('instances/delete', { name: 'missing' })).status).toBe(404);
    expect((await post('instances/delete', { id: 'not-an-id' })).status).toBe(400);
    expect((await post('instances/delete', { id: spare.id, name: 'spare' })).status).toBe(400);
    response = await post('instances/delete', { name: 'spare' });
    expect(response.status).toBe(200);
    const deleted = await response.json();
    expect(deleted.deleted.id).toBe(spare.id);
    expect(deleted.clusterDeleted).toBe(false);
    expect(deleted.removedDirectory).toBe(true);
    // No cluster existed, so only the detached registry container is removed (best effort).
    expect((await readFile(join(dir, 'k3d.log'), 'utf8')).trim().split('\n')).toEqual([expect.stringMatching(/^registry delete k3d-apeiron-[a-f0-9]{12}-reg$/)]);
    const listing = await fetch(server.url + 'api/instances').then(r => r.json());
    expect(listing.instances.map(i => i.name)).toEqual(['edge', 'lab']);
    expect((await post('instances/select', { id: edge.id })).status).toBe(200);
    expect((await fetch(server.url + 'api/instances/' + edge.id + '/status').then(r => r.json())).name).toBe('edge');
});
test('a packaged instance path is adopted on open and reported by api/config', async () => {
    const { post, config } = await fixture(home => join(home, '.apeiron/installers/cand/instances/default/config.json'));
    // The packaged layout is registered on open even before a configuration exists (phase "new").
    const snapshot = await config();
    expect(snapshot.instance).toMatchObject({ name: 'default', topology: 'single-k3d', httpPort: 54320, httpsPort: 54321, phase: 'new', exists: false });
    const saved = await post('config', { slug: 'example-team', apps: [...requiredApps], deployment: { ...(await config()).defaults, offline: false, bundleDir: '', installation: localInstallation() }, revision: null });
    expect(saved.status).toBe(200);
    const after = await config();
    expect(after.instance).toMatchObject({ name: 'default', topology: 'single-k3d', httpPort: 54320, httpsPort: 54321, phase: 'idle' });
    const created = await post('instances/create', { name: 'default', topology: 'single-k3d' }).then(r => r.json());
    expect(created.existing).toBe(true);
    expect(created.instance.id).toBe(after.instance.id);
});
test('api/preflight validates like deploy, reports warnings or a blocking reason, and never saves', async () => {
    const { path, post, body, installBody, setup } = await fixture();
    let response = await post('preflight', await installBody());
    expect(response.status).toBe(200);
    let result = await response.json();
    expect(result.installation).toMatchObject({ topology: 'single-k3d', httpPort: 54320, httpsPort: 54321 });
    expect(result.blocked).toBeNull();
    expect(typeof result.preflight).toBe('string');
    expect(await Bun.file(path).exists()).toBe(false);
    // A port mismatch with the owned cluster is reported as blocked, not as an HTTP error.
    await writeFile(join(setup.bin, 'docker'), '#!/bin/sh\nif [ "$1" = inspect ]; then echo \'{"cluster":"x","running":true,"bindings":{"54320/tcp":[{"HostIp":"127.0.0.1","HostPort":"54320"}],"443/tcp":[{"HostIp":"127.0.0.1","HostPort":"54399"}]}}\'; fi\nexit 0\n', { mode: 0o755 });
    response = await post('preflight', await installBody());
    expect(response.status).toBe(200);
    result = await response.json();
    expect(result.blocked).toMatch(/不属于本次安装|端口/);
    expect((await post('preflight', { ...(await installBody()), apps: ['apeiron', 'nope'] })).status).toBe(400);
    expect((await post('preflight', body())).status).toBe(400, 'legacy deployment without installation is refused');
    expect((await post('preflight', { slug: 'x', apps: [...requiredApps], revision: null })).status).toBe(400);
});
test('application catalog: browser and ci are optional, ci requires git, unknown apps are refused by name', async () => {
    const { post, config, body } = await fixture();
    const snapshot = await config();
    expect(snapshot.apps.map(a => a.id)).toEqual(['nexus', 'vasi', 'ontology', 'apeiron', 'task', 'corpus', 'zygos', 'matrix', 'files', 'stalwart', 'gateway', 'filer', 'git', 'gpustack', 'langfuse', 'kps', 'browser', 'ci']);
    expect(snapshot.apps.find(a => a.id === 'zygos')).toMatchObject({ selected: false, required: false });
    expect(snapshot.apps.find(a => a.id === 'ci')).toMatchObject({ selected: false, required: false, requires: ['git'], releases: ['woodpecker'] });
    expect(snapshot.apps.find(a => a.id === 'browser')).toMatchObject({ selected: false, required: false, releases: ['chrome-headless'] });
    let response = await post('config', { ...body(), apps: [...requiredApps, 'ci'] });
    expect(response.status).toBe(400);
    expect((await response.json()).error).toBe('CI 需要同时启用 代码仓库。');
    response = await post('config', { ...body(), apps: [...requiredApps, 'keycloak'] });
    expect(response.status).toBe(400);
    expect((await response.json()).error).toBe('应用目录不包含：keycloak。');
    response = await post('config', { ...body(), apps: [...requiredApps, 'browser', 'git', 'ci'] });
    expect(response.status).toBe(200);
    expect((await response.json()).config.apps).toEqual([...requiredApps, 'git', 'browser', 'ci']);
});
