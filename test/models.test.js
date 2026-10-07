import { test, expect } from 'bun:test';
import { mkdtemp, readFile, stat, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startInitServer } from './rust-server';
const models = { provider: 'bigmodel', baseUrl: 'https://models.example.internal/v1', apiKey: 'test-model-secret', fast: 'fast-model', deep: 'reasoning-model' };
const deployment = { runner: 'native', root: '', environment: '/tmp/environment.yaml', kubeconfig: '/tmp/kubeconfig', workDir: '', image: '', offline: true, bundleDir: '/tmp/bundle' };
const input = { slug: 'example', apps: ['nexus', 'vasi', 'ontology', 'apeiron'], deployment, models };
test('model discovery and both inference tests use the exact endpoint, bound output and never expose upstream errors', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'models-request-'));
    const server = await startInitServer({ path: join(dir, 'config.json') });
    const modelRequest = async (models, action) => {
        const response = await fetch(server.url + 'api/models/' + action, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ models, revision: null }) });
        const value = await response.json();
        if (!response.ok) throw new Error(value.error);
        return value;
    };
    const calls = [];
    const provider = Bun.serve({ hostname: '127.0.0.1', port: 0, async fetch(req) {
            calls.push({ path: new URL(req.url).pathname, auth: req.headers.get('authorization'), body: req.method === 'POST' ? await req.json() : null });
            return Response.json(req.method === 'GET' ? { data: [{ id: 'fast-model' }, { id: 'reasoning-model' }] } : { choices: [{ message: { content: 'OK' } }] });
        } });
    try {
        const configured = { ...models, baseUrl: `http://127.0.0.1:${provider.port}/v1` };
        expect(await modelRequest(configured, 'list', new AbortController().signal)).toEqual({ models: ['fast-model', 'reasoning-model'] });
        const result = await modelRequest(configured, 'test', new AbortController().signal);
        expect(result.results?.map(r => r.mode)).toEqual(['fast', 'deep']);
        expect(calls.map(c => c.path)).toEqual(['/v1/models', '/v1/chat/completions', '/v1/chat/completions']);
        expect(calls[1].body.model).toBe(models.fast);
        expect(calls[2].body.model).toBe(models.deep);
        expect(calls.every(c => c.auth === `Bearer ${models.apiKey}`)).toBe(true);
        provider.reload({ fetch: () => Response.json({ data: [] }) });
        await expect(modelRequest(configured, 'list', new AbortController().signal)).rejects.toThrow('未返回可用模型');
        provider.reload({ fetch: () => new Response(models.apiKey, { status: 401 }) });
        await expect(modelRequest(configured, 'list', new AbortController().signal)).rejects.toThrow('HTTP 401');
        provider.reload({ fetch: () => Response.redirect(`http://127.0.0.1:${provider.port}/other`, 302) });
        await expect(modelRequest(configured, 'test', new AbortController().signal)).rejects.toThrow('无法完成模型请求');
    }
    finally {
        provider.stop(true);
        await server.stop();
        await rm(dir, { recursive: true, force: true });
    }
});
test('model routes require same origin and current config revision', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'setup-model-api-'));
    const server = await startInitServer({ path: join(dir, 'config.json') });
    try {
        expect((await fetch(server.url + 'api/models/test', { method: 'POST', headers: { Origin: 'https://other.example', 'Content-Type': 'application/json' }, body: JSON.stringify({ models, revision: null }) })).status).toBe(403);
        expect((await fetch(server.url + 'api/models/list', { method: 'POST', headers: { Origin: new URL(server.url).origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ models, revision: 'stale' }) })).status).toBe(409);
    }
    finally {
        await server.stop();
        await rm(dir, { recursive: true, force: true });
    }
});
test('wizard model HTTP boundary preserves responses, redacts errors, and cancels a stalled provider on shutdown', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'setup-model-transport-'));
    const provider = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: () => Response.json({ data: [{ id: 'fast-model' }] }) });
    const server = await startInitServer({ path: join(dir, 'config.json') });
    const configured = { ...models, baseUrl: `http://127.0.0.1:${provider.port}/v1` };
    const post = () => fetch(server.url + 'api/models/list', { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ models: configured, revision: null }) });
    let release;
    try {
        expect(await (await post()).json()).toEqual({ models: ['fast-model'] });
        provider.reload({ fetch: () => new Response(models.apiKey, { status: 401 }) });
        const denied = await post();
        expect(denied.status).toBe(400);
        const body = await denied.text();
        expect(body).toContain('HTTP 401');
        expect(body).not.toContain(models.apiKey);
        let received;
        const started = new Promise(resolve => { received = resolve; });
        provider.reload({ async fetch() { received(); await new Promise(resolve => { release = resolve; }); return Response.json({ data: [] }); } });
        const pending = post().then(r => r.text()).catch(() => 'closed');
        await started;
        const before = Date.now();
        await server.stop();
        expect(Date.now() - before).toBeLessThan(3000);
        await pending;
    }
    finally {
        release?.();
        provider.stop(true);
        await server.stop();
        await rm(dir, { recursive: true, force: true });
    }
});
