import { expect, test } from 'bun:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { localInstallation as installationDefaults } from './support';
const access = { domain: 'example.internal', entryIp: '127.0.0.1' };
import { startInitServer } from './rust-server';
import { deploymentFixture, freshInstallFixture, requiredApps } from './fixtures';
test('credential endpoint requires a successful deployment, same-origin POST and no parameters; general responses/logs never contain passwords', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-verification-api-'));
    const setup = await deploymentFixture(dir);
    await freshInstallFixture(dir, setup, 0, access.domain);
    const oldPath = process.env.PATH;
    process.env.PATH = setup.bin + ':' + oldPath;
    const server = await startInitServer({ path: join(dir, 'config.json'), env: { APEIRON_CHENTU_ROOT: setup.root } });
    const post = (endpoint, body = {}, origin = server.origin) => fetch(server.url + 'api/' + endpoint, { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    try {
        expect((await post('credentials')).status).toBe(409);
        expect((await fetch(server.url + 'api/credentials')).status).toBe(404);
        expect((await post('credentials', {}, 'https://untrusted.example')).status).toBe(403);
        expect((await post('verification')).status).toBe(409);
        expect((await post('deploy', { slug: 'fixture', apps: requiredApps, revision: null, deployment: { offline: false,
                installation: { ...installationDefaults(), topology: 'single-k3d', httpPort: 54320, httpsPort: 54321, domain: access.domain, entryIp: access.entryIp } } })).status).toBe(202);
        for (let i = 0; i < 400 && server.result.phase !== 'succeeded' && server.result.phase !== 'failed'; i++)
            await Bun.sleep(10);
        expect(server.result.phase).toBe('succeeded');
        expect((await post('credentials', { path: '/anywhere', secret: 'other' })).status).toBe(400);
        expect((await post('verification', { domain: 'attacker.internal' })).status).toBe(400);
        const response = await post('credentials');
        expect(response.status).toBe(200);
        expect(response.headers.get('cache-control')).toBe('no-store');
        const admin = await response.json();
        expect(admin).toEqual({ username: 'fixture-admin', password: 'fixture-only-admin-password' });
        for (const endpoint of ['config', 'deployment', 'access', 'verification', 'log']) {
            expect(await fetch(server.url + 'api/' + endpoint).then(r => r.text())).not.toContain(admin.password);
        }
        expect(await readFile(server.result.log, 'utf8')).not.toContain(Buffer.from(admin.password).toString('base64'));
        const checked = await post('verification').then(r => r.json());
        expect(checked.result.passed).toBe(false); // This isolated fixture has no trusted platform DNS/TLS.
        expect(checked.result.checks.map(c => c.name)).toEqual(['Apeiron', 'Apeiron Ops', 'IAM']);
        expect((await fetch(server.url + 'api/verification').then(r => r.json())).result).toEqual(checked.result);
    }
    finally {
        await server.stop();
        process.env.PATH = oldPath;
        await rm(dir, { recursive: true, force: true });
    }
}, 10_000);
