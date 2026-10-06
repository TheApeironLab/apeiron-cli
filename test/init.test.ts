import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, readFile, rm, stat, symlink, writeFile, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ConfigStore } from '../src/init/config';
import { startInitServer } from '../src/init/server';

const cleanups: Array<() => Promise<unknown>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0).reverse()) await cleanup(); });
const input = {
  slug: 'example-team', llm: { baseUrl: 'https://models.example.internal/v1', modelId: 'example-model', apiKey: 'test-only-key' },
  apps: ['ontology', 'apeiron'], revision: null,
};
async function fixture() {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-init-test-'));
  cleanups.push(() => rm(dir, { recursive: true, force: true }));
  const path = join(dir, 'config', 'config.json');
  const server = await startInitServer({ path });
  cleanups.push(server.stop);
  const post = (body: unknown, headers = {}) => fetch(server.url + 'api/config', {
    method: 'POST', headers: { 'Content-Type': 'application/json', Origin: server.origin, ...headers }, body: JSON.stringify(body),
  });
  return { dir, path, server, post };
}

test('wizard starts without creating config and serves local-only assets', async () => {
  const { path, server } = await fixture();
  expect(await Bun.file(path).exists()).toBe(false);
  const page = await fetch(server.url);
  expect(page.status).toBe(200);
  expect(page.headers.get('content-security-policy')).toContain("frame-ancestors 'none'");
  expect(page.headers.get('cache-control')).toBe('no-store');
  const html = await page.text();
  expect(html).toContain('Slug name');
  expect(html).not.toContain('<script src=');
  const snapshot = await fetch(server.url + 'api/config').then(r => r.json());
  expect(snapshot.config).toBeNull();
  expect(snapshot.apps.some((a: { id: string }) => a.id === 'ontology')).toBe(true);
  expect((await fetch(server.origin + '/')).status).toBe(404);
});

test('save round trip has private permissions, redacts keys, and supports retaining/removing them', async () => {
  const { path, server, post } = await fixture();
  const response = await post(input);
  expect(response.status).toBe(200);
  const raw = await response.text();
  expect(raw).not.toContain(input.llm.apiKey);
  const saved = JSON.parse(raw);
  expect(saved.config.llm.hasApiKey).toBe(true);
  expect((await stat(path)).mode & 0o777).toBe(0o600);
  expect((await stat(join(path, '..'))).mode & 0o777).toBe(0o700);
  expect(await Bun.file(path).json()).toEqual({ schemaVersion: 1, slug: input.slug, llm: input.llm, apps: ['apeiron', 'ontology'] });
  const retained = await post({ ...input, revision: saved.revision, llm: { baseUrl: input.llm.baseUrl, modelId: 'updated-model' } });
  expect(retained.status).toBe(200);
  const second = await retained.json();
  expect((await Bun.file(path).json()).llm.apiKey).toBe(input.llm.apiKey);
  const bootstrap = await fetch(server.url + 'api/config').then(r => r.text());
  expect(bootstrap).not.toContain(input.llm.apiKey);
  expect((await post({ ...input, revision: second.revision, llm: { ...input.llm, apiKey: '' } })).status).toBe(200);
  expect((await Bun.file(path).json()).llm.apiKey).toBe('');
});

test('validation and cross-origin requests never create a configuration', async () => {
  const { path, server, post } = await fixture();
  for (const body of [
    { ...input, slug: 'INVALID name' },
    { ...input, llm: { ...input.llm, baseUrl: 'javascript:alert(1)' } },
    { ...input, llm: { ...input.llm, baseUrl: 'https://user:secret@example.com/v1' } },
    { ...input, apps: [] }, { ...input, apps: ['unknown'] }, { ...input, apps: ['apeiron', 'apeiron'] },
  ]) expect((await post(body)).status).toBe(400);
  expect((await post(input, { Origin: 'https://example.com' })).status).toBe(403);
  expect((await fetch(server.url + 'api/config', { method: 'POST', body: JSON.stringify(input) })).status).toBe(403);
  expect((await fetch(server.url, { headers: { Host: 'example.com', 'Sec-Fetch-Site': 'cross-site' } })).status).toBe(403);
  expect(await Bun.file(path).exists()).toBe(false);
});

test('stale browser data cannot overwrite a newer configuration', async () => {
  const { path, post } = await fixture();
  expect((await post(input)).status).toBe(200);
  const before = await readFile(path, 'utf8');
  expect((await post({ ...input, slug: 'stale-edit' })).status).toBe(409);
  expect(await readFile(path, 'utf8')).toBe(before);
});

test('existing unsupported files and symlinks are preserved', async () => {
  const { dir } = await fixture();
  const existing = join(dir, 'existing.json');
  await writeFile(existing, '{"schemaVersion":99,"apiKey":"test-secret"}');
  await expect(startInitServer({ path: existing })).rejects.toThrow('版本不支持');
  expect(await readFile(existing, 'utf8')).toContain('test-secret');
  const link = join(dir, 'link.json');
  await symlink(existing, link);
  await expect(startInitServer({ path: link })).rejects.toThrow('普通文件');
  const repo = join(dir, 'repo');
  await mkdir(join(repo, '.git'), { recursive: true });
  await expect(new ConfigStore(join(repo, 'config.json')).checkLocation()).rejects.toThrow('Git');
});

test('finish drains the server after a successful save', async () => {
  const { server, post } = await fixture();
  const finish = () => fetch(server.url + 'api/finish', { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: '{}' });
  expect((await finish()).status).toBe(409);
  expect((await post(input)).status).toBe(200);
  expect((await finish()).status).toBe(200);
  await server.closed;
});
