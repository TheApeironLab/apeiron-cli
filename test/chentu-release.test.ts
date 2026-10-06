import { expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { CHENTU_RELEASE, ChentuResources, downloadChentu } from '../src/resources/chentu';
import { readInstallCatalog } from '../src/resources/install';

const signal = () => new AbortController().signal;
const quiet = () => {};

test('OSS requires a complete HTTP 200 response and never attaches credentials', async () => {
  for (const status of [204, 206, 403, 404, 500]) {
    await expect(downloadChentu(CHENTU_RELEASE, signal(), async () => new Response(null, { status }))).rejects.toThrow(`HTTP ${status}`);
  }
  const bytes = await downloadChentu(CHENTU_RELEASE, signal(), async (url, init) => {
    expect(url).toBe(CHENTU_RELEASE.url);
    expect(new Headers(init.headers).has('Authorization')).toBe(false);
    expect(init.redirect).toBe('manual');
    return new Response('fixture');
  });
  expect(new TextDecoder().decode(bytes)).toBe('fixture');
});

test('OSS rejects untrusted initial URLs and redirect destinations before contacting them', async () => {
  let calls = 0;
  const forbidden = ['http://example.internal/package', 'https://example.internal/package',
    CHENTU_RELEASE.url.replace('https://', 'https://user:password@'), CHENTU_RELEASE.url + '?token=fixture'];
  for (const url of forbidden) {
    await expect(downloadChentu({ ...CHENTU_RELEASE, url }, signal(), async () => { calls++; return new Response(); })).rejects.toThrow('不受信任');
  }
  expect(calls).toBe(0);
  await expect(downloadChentu(CHENTU_RELEASE, signal(), async () => {
    calls++;
    return new Response(null, { status: 302, headers: { location: 'https://example.internal/package' } });
  })).rejects.toThrow('不受信任');
  expect(calls).toBe(1);
});

test('pinned OSS package validates its digest, repairs cache, and remains blocked without an install catalog', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-oss-release-'));
  const prefix = 'chentu-' + CHENTU_RELEASE.version + '/';
  const files = Object.fromEntries(['deploy/helmfile/run.sh', 'deploy/helmfile/helmfile.yaml.gotmpl',
    'deploy/helmfile/scripts/check.py', 'cli/src/chentu/environment.py', 'tests/lab/helmfile.sh']
    .map(path => [prefix + path, 'fixture source']));
  files[prefix + 'chentu-package.json'] = JSON.stringify({ schemaVersion: 1, kind: 'chentu-deployment',
    version: CHENTU_RELEASE.version, revision: CHENTU_RELEASE.revision });
  try {
    const archivePath = join(directory, 'fixture.tar.gz');
    await Bun.Archive.write(archivePath, files, { compress: 'gzip' });
    const archive = await readFile(archivePath);
    const release = { ...CHENTU_RELEASE, sha256: createHash('sha256').update(archive).digest('hex') };
    let calls = 0;
    const resources = new ChentuResources(join(directory, 'cache'), release, async () => { calls++; return archive; });
    await expect(resources.resolve(signal(), quiet, { offline: true, bundleDir: '' })).rejects.toThrow('离线');
    expect(calls).toBe(0);
    const root = await resources.resolve(signal(), quiet);
    expect(calls).toBe(1);
    await expect(readInstallCatalog(root)).rejects.toThrow('setup/install.json');
    const entry = join(root, 'deploy/helmfile/run.sh');
    await writeFile(entry, 'modified');
    await resources.resolve(signal(), quiet, { offline: true, bundleDir: '' });
    expect(await readFile(entry, 'utf8')).toBe('fixture source');
    expect(calls).toBe(1);
    const corrupt = new ChentuResources(join(directory, 'corrupt'), release, async () => new TextEncoder().encode('corrupt'));
    await expect(corrupt.resolve(signal(), quiet)).rejects.toThrow('校验失败');
    const wrongRevision = new ChentuResources(join(directory, 'revision'), { ...release, revision: '0'.repeat(40) }, async () => archive);
    await expect(wrongRevision.resolve(signal(), quiet)).rejects.toThrow('版本与 CLI');
  } finally { await rm(directory, { recursive: true, force: true }); }
});
