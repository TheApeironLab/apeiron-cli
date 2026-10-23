// secrets: none
import { expect, test } from 'bun:test';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startInitServer } from './rust-server';
import { requiredApps } from './fixtures';
import { post } from './support';

test('unified K3s intent round-trips one or multiple nodes without changing apps or enabling HA', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-node-intent-'));
    const path = join(dir, 'config.json');
    let server = await startInitServer({ path });
    try {
        let revision = null;
        for (const count of [1, 2, 3]) {
            const nodes = Array.from({ length: count }, (_, n) => ({ host: `node-${n}`, name: `node-${n}`, address: `192.0.2.${10 + n}`, role: n === 0 ? 'server' : 'agent' }));
            const input = { slug: 'example', revision, apps: requiredApps, deployment: { offline: false, installation: {
                topology: 'k3s', domain: 'example.internal', entryIp: '192.0.2.10', httpPort: 80, httpsPort: 443,
                ha: false, sshUser: '', sshKey: '', sshPort: 22, nodes,
            } } };
            const response = await post(server, 'config', input);
            expect(response.status).toBe(200);
            revision = (await response.json()).revision;
            const saved = JSON.parse(await readFile(path, 'utf8'));
            expect(saved.deployment.installation.topology).toBe('k3s');
            expect(saved.deployment.installation.nodes).toEqual(nodes);
            expect(saved.deployment.installation.ha).toBe(false);
            expect(saved.apps).toEqual(requiredApps);
            await server.stop();
            server = await startInitServer({ path });
            const snapshot = await fetch(server.url + 'api/config').then(r => r.json());
            expect(snapshot.config.deployment.installation).toEqual(saved.deployment.installation);
        }
    } finally {
        await server.stop();
        await rm(dir, { recursive: true, force: true });
    }
});
