import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, readFile, rm, stat, symlink, writeFile, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ConfigStore } from '../src/init/config';
import { startInitServer } from '../src/init/server';
import { deploymentFixture, requiredApps } from './fixtures';

const cleanups: Array<() => Promise<unknown>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0).reverse()) await cleanup(); });
async function fixture() {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-init-test-'));
  cleanups.push(() => rm(dir, { recursive: true, force: true }));
  const setup = await deploymentFixture(dir);
  const oldPath = process.env.PATH;
  process.env.PATH = setup.bin + ':' + oldPath;
  cleanups.push(async () => { process.env.PATH = oldPath; });
  const path = join(dir, 'config', 'config.json');
  const server = await startInitServer({ path });
  cleanups.push(server.stop);
  const input = { slug: 'example-team', apps: [...requiredApps].reverse(), deployment: setup.target, revision: null };
  const post = (body: unknown, headers = {}, endpoint = 'config') => fetch(server.url + 'api/' + endpoint, {
    method: 'POST', headers: { 'Content-Type': 'application/json', Origin: server.origin, ...headers }, body: JSON.stringify(body),
  });
  const status = () => fetch(server.url + 'api/deployment').then(r => r.json());
  return { ...setup, dir, path, server, input, post, status };
}
async function until<T>(get: () => Promise<T>, done: (value: T) => boolean): Promise<T> {
  for (let n = 0; n < 300; n++) { const value = await get(); if (done(value)) return value; await Bun.sleep(10); }
  throw new Error('Timed out waiting for deployment');
}

test('wizard serves two steps, complete catalog and no model fields without writing config', async () => {
  const { path, server } = await fixture();
  expect(await Bun.file(path).exists()).toBe(false);
  const page = await fetch(server.url);
  expect(page.headers.get('content-security-policy')).toContain("frame-ancestors 'none'");
  expect(page.headers.get('cache-control')).toBe('no-store');
  const html = await page.text();
  expect(html).toContain('共 2 步');
  expect(html).not.toContain('id="base-url"');
  expect(html).not.toContain('id="api-key"');
  expect(html).not.toContain('<script src=');
  const snapshot = await fetch(server.url + 'api/config').then(r => r.json());
  expect(snapshot.config).toBeNull();
  expect(snapshot.apps.map((a: { id: string }) => a.id)).toEqual([
    'vasi', 'apeiron', 'ontology', 'task', 'corpus', 'matrix', 'files', 'gateway', 'nexus',
    'filer', 'stalwart', 'git', 'gpustack', 'langfuse', 'kps',
  ]);
  expect(snapshot.apps.filter((a: { required: boolean }) => a.required).length).toBe(8);
  expect((await fetch(server.origin + '/')).status).toBe(404);
});

test('configuration round trip requires deployment settings and private permissions', async () => {
  const { path, input, post } = await fixture();
  const response = await post(input);
  expect(response.status).toBe(200);
  expect((await stat(path)).mode & 0o777).toBe(0o600);
  expect((await stat(join(path, '..'))).mode & 0o777).toBe(0o700);
  expect(await Bun.file(path).json()).toEqual({ schemaVersion: 2, slug: input.slug, deployment: input.deployment, apps: requiredApps });
});

test('invalid inputs, cross-origin requests and missing required apps cannot write config', async () => {
  const { path, server, input, post } = await fixture();
  for (const body of [
    { ...input, slug: 'INVALID name' }, { ...input, deployment: undefined },
    { ...input, deployment: { ...input.deployment, environment: 'relative.yaml' } },
    { ...input, deployment: { ...input.deployment, runner: 'shell' } },
    { ...input, deployment: { ...input.deployment, image: '--privileged' } },
    { ...input, apps: [] }, { ...input, apps: ['unknown'] }, { ...input, apps: ['apeiron', 'apeiron'] },
    ...requiredApps.map(id => ({ ...input, apps: requiredApps.filter(app => app !== id) })),
  ]) expect((await post(body)).status).toBe(400);
  expect((await post(input, { Origin: 'https://example.com' }, 'deploy')).status).toBe(403);
  expect((await fetch(server.url + 'api/deploy', { method: 'POST', body: JSON.stringify(input) })).status).toBe(403);
  expect((await fetch(server.url, { headers: { Host: 'example.com', 'Sec-Fetch-Site': 'cross-site' } })).status).toBe(403);
  expect(await Bun.file(path).exists()).toBe(false);
});

test('stale browser data cannot overwrite a newer configuration or start deployment', async () => {
  const { path, input, post, calls } = await fixture();
  expect((await post(input)).status).toBe(200);
  const before = await readFile(path, 'utf8');
  expect((await post({ ...input, slug: 'stale-edit' }, {}, 'deploy')).status).toBe(409);
  expect(await readFile(path, 'utf8')).toBe(before);
  expect(await Bun.file(calls).exists()).toBe(false);
});

test('legacy model credentials stay on disk, never appear in browser responses, and need no new model fields', async () => {
  const { dir, input } = await fixture();
  const path = join(dir, 'legacy.json');
  const llm = { baseUrl: 'https://models.example.internal/v1', modelId: 'old-model', apiKey: 'test-legacy-secret' };
  const legacy = JSON.stringify({ schemaVersion: 1, slug: input.slug, llm, apps: ['apeiron', 'ontology', 'filer'] });
  await writeFile(path, legacy, { mode: 0o600 });
  const server = await startInitServer({ path }); cleanups.push(server.stop);
  const response = await fetch(server.url + 'api/config').then(r => r.text());
  expect(response).not.toContain(llm.apiKey);
  expect(response).not.toContain('old-model');
  expect(await readFile(path, 'utf8')).toBe(legacy);
  const store = new ConfigStore(path);
  await store.save({ ...input, revision: (await store.read()).revision, apps: [...requiredApps, 'filer'] });
  expect((await store.read()).config?.llm).toEqual(llm);
  expect((await store.read()).config?.schemaVersion).toBe(2);
});

test('unsupported files, symlinks and Git config destinations remain protected', async () => {
  const { dir } = await fixture();
  const existing = join(dir, 'existing.json');
  await writeFile(existing, '{"schemaVersion":99,"apiKey":"test-secret"}');
  await expect(startInitServer({ path: existing })).rejects.toThrow('版本不支持');
  const link = join(dir, 'link.json'); await symlink(existing, link);
  await expect(startInitServer({ path: link })).rejects.toThrow('普通文件');
  const repo = join(dir, 'repo'); await mkdir(join(repo, '.git'), { recursive: true });
  await expect(new ConfigStore(join(repo, 'config.json')).checkLocation()).rejects.toThrow('Git');
});

test('deploy generates exact app flags, preserves base values, runs sync once and survives reload', async () => {
  const { input, post, status, calls, environment, source, server } = await fixture();
  expect((await post(input, {}, 'deploy')).status).toBe(202);
  expect((await post(input, {}, 'deploy')).status).toBe(409);
  expect((await post(input)).status).toBe(409);
  expect((await post({}, {}, 'finish')).status).toBe(409);
  await until(status, s => s.phase === 'running');
  const bootstrap = await fetch(server.url + 'api/config').then(r => r.json());
  expect(bootstrap.deployment.phase).toBe('running');
  const done = await until(status, s => s.phase === 'succeeded');
  expect(done.exitCode).toBe(0);
  expect(JSON.stringify(done)).not.toContain('private-log-test-key');
  expect(await readFile(done.log, 'utf8')).toContain('private-log-test-key');
  expect((await stat(done.log)).mode & 0o777).toBe(0o600);
  expect((await stat(done.environment)).mode & 0o777).toBe(0o600);
  const records = (await readFile(calls, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
  expect(records.length).toBe(1);
  expect(records[0].args).toEqual(['sync']);
  expect(records[0].values.tenantSlug).toBe(input.slug);
  expect(records[0].values.releases.apeiron.values.models).toEqual({ keep: true });
  expect(records[0].values.releases.postgres).toEqual({ enabled: true, values: { storage: '10Gi' } });
  expect(records[0].values.releases.stalwart.enabled).toBe(true);
  expect(records[0].values.releases.nexus.enabled).toBe(true);
  for (const name of ['kps', 'loki', 'promtail']) expect(records[0].values.releases[name].enabled).toBe(false);
  expect(await readFile(environment, 'utf8')).toBe(source);
  expect((await post({}, {}, 'finish')).status).toBe(200);
  await server.closed;
});

test('failed native deployment exposes exit code, supports retry and never fabricates success', async () => {
  const { input, post, status, source, environment, workDir, calls } = await fixture();
  const native = { ...input, deployment: { ...input.deployment, runner: 'native', kubeconfig: join(workDir, 'state/kubeconfig'), profile: 'ubuntu' } };
  await writeFile(environment, source.replace('fixtureExit: 0', 'fixtureExit: 17'));
  const first = await post(native, {}, 'deploy').then(r => r.json());
  const failed = await until(status, s => s.phase === 'failed');
  expect(failed.exitCode).toBe(17);
  await writeFile(environment, source);
  expect((await post({ ...native, revision: first.revision }, {}, 'deploy')).status).toBe(202);
  expect((await until(status, s => s.phase === 'succeeded')).exitCode).toBe(0);
  const record = JSON.parse((await readFile(calls, 'utf8')).trim().split('\n')[0]!);
  expect(record.profile).toBe('ubuntu');
  expect(record.kubeconfig).toBe(join(workDir, 'state/kubeconfig'));
});

test('missing environment fails before spawning and stopping cancels active child', async () => {
  const { input, post, status, calls, source, environment, server } = await fixture();
  const missing = await post({ ...input, deployment: { ...input.deployment, environment: join(environment, 'missing') } }, {}, 'deploy').then(r => r.json());
  expect((await until(status, s => s.phase === 'failed')).message).toContain('环境 values');
  expect(await Bun.file(calls).exists()).toBe(false);
  await writeFile(environment, source.replace('fixtureDelay: 700', 'fixtureDelay: 30000'));
  await post({ ...input, revision: missing.revision }, {}, 'deploy');
  await until(status, s => s.phase === 'running');
  await server.stop();
  expect(server.result.phase).toBe('cancelled');
  expect(await Bun.file(environment + '.apeiron-deploy.lock').exists()).toBe(false);
});

test('separate wizard sessions cannot deploy the same environment concurrently', async () => {
  const { dir, input, post, status, server } = await fixture();
  await post(input, {}, 'deploy');
  await until(status, s => s.phase === 'running');
  const second = await startInitServer({ path: join(dir, 'second.json') }); cleanups.push(second.stop);
  expect((await fetch(second.url + 'api/deploy', { method: 'POST', headers: { Origin: second.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(input) })).status).toBe(202);
  const failed = await until(async () => second.result, s => s.phase === 'failed');
  expect(failed.message).toContain('部署锁');
  await server.stop();
});
