import { startInitServer } from './rust-server';
import { expect, test } from 'bun:test';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, symlink } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { deploymentFixture } from './fixtures';
test('DNS and download routes keep origin and token protections and never accept arbitrary paths', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'apeiron-dns-api-'));
    const server = await startInitServer({ path: join(directory, 'config.json') });
    try {
        const post = (origin, body) => fetch(server.url + 'api/dns', { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
        expect((await post('https://untrusted.example', {})).status).toBe(403);
        expect((await post(server.origin, { domain: 'bad/host', entryIp: '127.0.0.1', local: true })).status).toBe(400);
        for (const file of ['ca.crt', 'hosts.txt', 'ca.crt?path=/etc/passwd', 'ca.key'])
            expect((await fetch(server.url + 'api/' + file)).status).toBe(404);
        const config = await fetch(server.url + 'api/config').then(response => response.json());
        expect(typeof config.host.name).toBe('string');
        expect(Array.isArray(config.host.addresses)).toBe(true);
    }
    finally {
        await server.stop();
        await rm(directory, { recursive: true, force: true });
    }
});
