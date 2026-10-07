import { test, expect } from 'bun:test';
import { mkdtemp, rm, stat, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { binary } from './support';
// Crosses enrollment HTTP and the authenticated WebSocket tunnel.
test('Rust registration keeps credentials local and enforces gateway slug through the tunnel', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-rust-register-'));
    const id = 'aefcb762-56b7-4ff1-8969-5dd5a0895647';
    const token = 'T'.repeat(43);
    const replies = new Map();
    let socket;
    let enrollments = 0;
    const server = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: async (request, server) => {
            const url = new URL(request.url);
            if (url.pathname === '/v1/agent/enroll') {
                expect(await request.json()).toEqual({ token: 'E'.repeat(43) });
                enrollments++;
                return Response.json({ deployment_id: id, agent_token: token, slug: 'assigned', domain: 'assigned.example.test' });
            }
            expect(request.headers.get('authorization')).toBe(`Bearer ${token}`);
            if (server.upgrade(request))
                return;
            return new Response('not found', { status: 404 });
        }, websocket: { open: ws => { socket = ws; }, message: (_ws, data) => { const message = JSON.parse(String(data)); replies.get(message.id)?.(message); } } });
    const child = Bun.spawn([binary, 'register', '--gateway', `http://127.0.0.1:${server.port}`, '--enrollment-token', 'E'.repeat(43)], { env: { ...process.env, HOME: dir }, stdout: 'pipe', stderr: 'pipe' });
    const call = async (path, method = 'GET', body = {}) => {
        const id = crypto.randomUUID();
        const result = new Promise((resolve, reject) => {
            const timer = setTimeout(() => reject(new Error('Tunnel response timed out')), 10000);
            replies.set(id, value => { clearTimeout(timer); replies.delete(id); resolve(value); });
        });
        socket.send(JSON.stringify({ type: 'request', id, method, path, body: method === 'POST' ? JSON.stringify(body) : '', headers: { cookie: 'must-not-be-forwarded', host: 'untrusted.invalid' } }));
        const response = await result;
        return { status: response.status, body: Buffer.from(response.body, 'base64').toString() };
    };
    try {
        for (let i = 0; !socket && i < 200; i++)
            await Bun.sleep(25);
        expect(Boolean(socket)).toBe(true);
        const config = await call('api/config');
        expect(config.status).toBe(200);
        const snapshot = JSON.parse(config.body);
        expect(snapshot.gateway).toEqual({ slug: 'assigned' });
        expect(config.body).not.toContain(token);
        const altered = await call('api/config', 'POST', { slug: 'different', apps: ['apeiron'], revision: null });
        expect(altered.status).toBe(400);
        expect(altered.body).toContain('组织标识');
        const accepted = await call('api/config', 'POST', { slug: 'assigned', apps: ['nexus', 'vasi', 'ontology', 'apeiron'], revision: snapshot.revision, deployment: { runner: 'native', root: '', environment: '/tmp/fixture-environment.yaml', kubeconfig: '/tmp/fixture-kubeconfig', workDir: '', image: '', offline: false, bundleDir: '' } });
        expect(accepted.status).toBe(200);

        const page = await call('');
        expect(page.status).toBe(200);
        expect(page.body.includes('id="model-fast"')).toBe(true);
        expect(enrollments).toBe(1);
        const saved = join(dir, '.apeiron/gateway', `${id}.json`);
        expect((await stat(saved)).mode & 0o777).toBe(0o600);
        expect(JSON.parse(await readFile(saved, 'utf8')).agent_token).toBe(token);
        socket.close(4001, 'replaced');
        expect(await child.exited).toBe(0);
        const logs = await new Response(child.stdout).text() + await new Response(child.stderr).text();
        expect(logs).not.toContain(token);
        expect(logs).not.toMatch(/\/setup\/[a-f0-9]+/);
    }
    finally {
        child.kill();
        await child.exited;
        server.stop(true);
        await rm(dir, { recursive: true, force: true });
    }
}, 20000);
