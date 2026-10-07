import { test, expect } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startInitServer } from './rust-server';
test('probe endpoint requires same-origin POST and explicit mode, respects offline and never saves configuration', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-probe-api-'));
    const path = join(dir, 'config.json');
    let calls = 0;
    const proxy = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: () => { calls++; return new Response('', { status: 403 }); } });
    const proxyUrl = `http://127.0.0.1:${proxy.port}`;
    const server = await startInitServer({ path, env: { HTTPS_PROXY: proxyUrl, HTTP_PROXY: proxyUrl, ALL_PROXY: proxyUrl, https_proxy: proxyUrl, http_proxy: proxyUrl, NO_PROXY: '', no_proxy: '' } });
    try {
        const post = (body, origin = server.origin) => fetch(server.url + 'api/probe', {
            method: 'POST', headers: { 'Content-Type': 'application/json', Origin: origin }, body: JSON.stringify(body),
        });
        expect((await fetch(server.url + 'api/probe')).status).toBe(404);
        expect((await post({ offline: false }, 'https://example.com')).status).toBe(403);
        for (const body of [{}, { offline: 'false' }, { offline: false, refresh: 'yes' }, null])
            expect((await post(body)).status).toBe(400);
        const offline = await post({ offline: true }).then(response => response.json());
        expect(offline.network.status).toBe('skipped');
        expect(['amd64', 'arm64']).toContain(offline.machine.hardware.architecture);
        expect(offline.machine.hardware.cores).toBeGreaterThan(0);
        expect(calls).toBe(0);
        const online = await post({ offline: false }).then(response => response.json());
        expect(online.network.status).toBe('unreachable');
        expect(calls).toBe(4);
        expect(await Bun.file(path).exists()).toBe(false);
    }
    finally {
        await server.stop();
        proxy.stop(true);
        await rm(dir, { recursive: true, force: true });
    }
});
