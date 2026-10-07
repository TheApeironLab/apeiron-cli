import { test, expect } from 'bun:test';
import { mkdtemp, readFile, stat, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ConfigStore, publicSnapshot, validateConfig } from '../src/init/config';
import { environmentFor } from '../src/init/deploy';
import { installModelKey, modelConfiguration, modelRequest } from '../src/init/models';
import { startInitServer } from './rust-server';

const models = { provider: 'bigmodel', baseUrl: 'https://models.example.internal/v1', apiKey: 'test-model-secret', fast: 'fast-model', deep: 'reasoning-model' };
const deployment = { runner: 'native' as const, root: '', environment: '/tmp/environment.yaml', kubeconfig: '/tmp/kubeconfig', workDir: '', image: '', offline: true, bundleDir: '/tmp/bundle' };
const input = { slug: 'example', apps: ['nexus', 'vasi', 'ontology', 'apeiron'], deployment, models };

test('model keys persist privately, redact on reload, preserve only for the same endpoint, and clear explicitly', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'setup-models-'));
  try {
    const store = new ConfigStore(join(dir, 'config.json'));
    const saved = await store.save({ ...input, revision: null });
    expect((await stat(store.path)).mode & 0o777).toBe(0o600);
    expect(JSON.stringify(publicSnapshot(saved))).not.toContain(models.apiKey);
    expect(publicSnapshot(saved).config?.models?.hasApiKey).toBe(true);
    const { apiKey, ...publicModel } = models;
    expect(validateConfig({ ...input, models: publicModel }, saved.config).models?.apiKey).toBe(apiKey);
    expect(() => validateConfig({ ...input, models: { ...publicModel, baseUrl: 'https://other.example/v1' } }, saved.config)).toThrow();
    expect(validateConfig({ ...input, models: { ...publicModel, apiKey: '' } }, saved.config).models?.apiKey).toBe('');
    expect(validateConfig({ ...input, models: null }, saved.config).models).toBeUndefined();
    for (const provider of ['', 'Big Model', '../invalid']) expect(() => modelConfiguration({ ...models, provider })).toThrow();
    expect(() => modelConfiguration({ ...models, provider: undefined })).toThrow();
    for (const baseUrl of ['file:///etc/passwd', 'https://user:pass@host/v1', 'https://host/v1?key=secret', 'https://host/#secret']) expect(() => modelConfiguration({ ...models, baseUrl })).toThrow();
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('deployment replaces deferred models with both modes and queues without placing keys in values', () => {
  const output = environmentFor(validateConfig(input), 'releases:\n  apeiron:\n    values:\n      models: {deferred: true}\n      keep: true\n');
  expect(output).not.toContain(models.apiKey);
  const values = (Bun.YAML.parse(output) as any).releases.apeiron.values;
  expect(values.keep).toBe(true);
  expect(values.allowedModels).toBe('');
  expect(values.defaultModel).toBe('');
  expect(values.models.deferred).toBeUndefined();
  expect(values.models.modes).toEqual({ fast: 'apeiron-flash', deep: 'apeiron-pro' });
  expect(values.models.queues.map((q: any) => q.model)).toEqual([models.fast, models.deep]);
  const same = (Bun.YAML.parse(environmentFor(validateConfig({ ...input, models: { ...models, deep: models.fast } }), '{}')) as any).releases.apeiron.values.models;
  expect(same.models).toHaveLength(2); expect(same.modes.deep).toBe('apeiron-pro'); expect(same.queues).toHaveLength(1);
  expect(values.models.providers[0].id).toBe('bigmodel');
  expect(values.models.models.map((m: any) => [m.id, m.provider, m.model])).toEqual([['apeiron-flash', 'bigmodel', models.fast], ['apeiron-pro', 'bigmodel', models.deep]]);
});

test('model discovery and both inference tests use the exact endpoint, bound output and never expose upstream errors', async () => {
  const calls: any[] = [];
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
    expect(calls[1].body.model).toBe(models.fast); expect(calls[2].body.model).toBe(models.deep);
    expect(calls.every(c => c.auth === `Bearer ${models.apiKey}`)).toBe(true);
    provider.reload({ fetch: () => Response.json({ data: [] }) });
    await expect(modelRequest(configured, 'list', new AbortController().signal)).rejects.toThrow('未返回可用模型');
    provider.reload({ fetch: () => new Response(models.apiKey, { status: 401 }) });
    await expect(modelRequest(configured, 'list', new AbortController().signal)).rejects.toThrow('HTTP 401');
    provider.reload({ fetch: () => Response.redirect(`http://127.0.0.1:${provider.port}/other`, 302) });
    await expect(modelRequest(configured, 'test', new AbortController().signal)).rejects.toThrow('无法完成模型请求');
  } finally { provider.stop(true); }
});

test('model routes require same origin and current config revision', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'setup-model-api-'));
  const server = await startInitServer({ path: join(dir, 'config.json') });
  try {
    expect((await fetch(server.url + 'api/models/test', { method: 'POST', headers: { Origin: 'https://other.example', 'Content-Type': 'application/json' }, body: JSON.stringify({ models, revision: null }) })).status).toBe(403);
    expect((await fetch(server.url + 'api/models/list', { method: 'POST', headers: { Origin: new URL(server.url).origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ models, revision: 'stale' }) })).status).toBe(409);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('native and Docker model credentials travel only via stdin, with failures redacted', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'setup-model-secret-'));
  try {
    for (const name of ['kubectl', 'docker']) await writeFile(join(dir, name), '#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE.args"\ncat > "$CAPTURE.stdin"\necho "secret diagnostic" >&2\nexit "${FAIL:-0}"\n', { mode: 0o700 });
    const env = { ...process.env, PATH: dir + ':' + process.env.PATH, CAPTURE: join(dir, 'capture'), LAB_CONTAINER: 'setup-owned', LAB_CLUSTER: 'setup-test' };
    for (const runner of ['native', 'docker'] as const) {
      await installModelKey(models, { ...deployment, runner, workDir: '/tmp/work', image: 'toolbox' }, env, new AbortController().signal);
      const args = await readFile(env.CAPTURE + '.args', 'utf8');
      expect(args).not.toContain(models.apiKey); expect(args).toContain('--server-side');
      const sent = JSON.parse(await readFile(env.CAPTURE + '.stdin', 'utf8'));
      expect(sent.items[1].metadata.name).toBe('apeiron-model-keys');
      expect(Buffer.from(sent.items[1].data.APEIRON_MODEL_API_KEY_SETUP, 'base64').toString()).toBe(models.apiKey);
      expect(sent.items[2].metadata.name).toBe('apeiron-scode-creds');
      expect(Buffer.from(sent.items[2].data.MODEL_API_KEY_SETUP, 'base64').toString()).toBe(models.apiKey);
      if (runner === 'docker') { expect(args).toContain('--pull=never'); expect(args).toContain('/tmp/work/state/kubeconfig:/kubeconfig:ro'); }
    }
    await expect(installModelKey(models, deployment, { ...env, FAIL: '1' }, new AbortController().signal)).rejects.toThrow('无法写入模型 Secret');
  } finally { await rm(dir, { recursive: true, force: true }); }
});


test('wizard model HTTP boundary preserves responses, redacts errors, and cancels a stalled provider on shutdown', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'setup-model-transport-'));
  const provider = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: () => Response.json({ data: [{ id: 'fast-model' }] }) });
  const server = await startInitServer({ path: join(dir, 'config.json') });
  const configured = { ...models, baseUrl: `http://127.0.0.1:${provider.port}/v1` };
  const post = () => fetch(server.url + 'api/models/list', { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ models: configured, revision: null }) });
  let release: (() => void) | undefined;
  try {
    expect(await (await post()).json()).toEqual({ models: ['fast-model'] });
    provider.reload({ fetch: () => new Response(models.apiKey, { status: 401 }) });
    const denied = await post();
    expect(denied.status).toBe(400);
    const body = await denied.text();
    expect(body).toContain('HTTP 401');
    expect(body).not.toContain(models.apiKey);
    let received!: () => void;
    const started = new Promise<void>(resolve => { received = resolve; });
    provider.reload({ async fetch() { received(); await new Promise<void>(resolve => { release = resolve; }); return Response.json({ data: [] }); } });
    const pending = post().then(r => r.text()).catch(() => 'closed');
    await started;
    const before = Date.now();
    await server.stop();
    expect(Date.now() - before).toBeLessThan(3000);
    await pending;
  } finally {
    release?.(); provider.stop(true); await server.stop(); await rm(dir, { recursive: true, force: true });
  }
});
