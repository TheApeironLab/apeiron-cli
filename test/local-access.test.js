import { test, expect } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { startInitServer } from './rust-server';
test('web install rejects cross-origin, missing tokens, parameters and incomplete deployments without prompting', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'access-api-'));
    const server = await startInitServer({ path: join(directory, 'config.json') });
    try {
        const post = (url, origin, body) => fetch(url, { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
        expect((await post(server.url + 'api/access/install', 'https://untrusted.example', {})).status).toBe(403);
        expect((await post(server.origin + '/setup/wrong/api/access/install', server.origin, {})).status).toBe(404);
        expect((await post(server.url + 'api/access/install', server.origin, {})).status).toBe(409);
        expect((await post(server.url + 'api/access/install', server.origin, { path: '/tmp/ca.crt' })).status).toBe(409);

        const state = await fetch(server.url + 'api/access').then(response => response.json());
        expect(state.status.phase).toBe('idle');
        expect(state.capability.host.length).toBeGreaterThan(0);
    }
    finally {
        await server.stop();
        await rm(directory, { recursive: true, force: true });
    }
});
