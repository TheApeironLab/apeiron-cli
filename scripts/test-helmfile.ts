// Read-only integration with real Helmfile; no Docker socket, network, kubeconfig or sync.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { APPS, type Configuration } from '../src/init/config';
import { environmentFor } from '../src/init/deploy';
const [rootArg, image] = process.argv.slice(2);
if (!rootArg || !image) throw new Error('Usage: bun scripts/test-helmfile.ts /path/to/chentu toolbox-image');
const root = resolve(rootArg);
const toolboxImage = image;
const dir = await mkdtemp(join(tmpdir(), 'apeiron-helmfile-check-'));
try {
  const source = await readFile(join(root, 'deploy/helmfile/environment.example.yaml'), 'utf8');
  const config: Configuration = { schemaVersion: 2, slug: 'integration-team', apps: APPS.filter(app => app.selected).map(app => app.id) };
  await writeFile(join(dir, 'environment.yaml'), environmentFor(config, source), { mode: 0o600 });
  async function helmfile(command: string) {
    const child = Bun.spawn(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', 'helmfile',
      '-v', root + ':/repo:ro', '-v', dir + ':/input:ro', '-w', '/repo/deploy/helmfile',
      '-e', 'CHENTU_ENV=/input/environment.yaml', toolboxImage,
      '--environment', 'single-k3d', '--file', 'helmfile.yaml.gotmpl', command], { stdout: 'pipe', stderr: 'pipe' });
    const [out, err, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
    assert.equal(code, 0, err);
    return Bun.YAML.parse(out) as any;
  }
  const resolved = (await helmfile('print-env')).values;
  assert.equal(resolved.tenantSlug, config.slug);
  for (const app of APPS) assert.equal(resolved.releases[app.id].enabled, config.apps.includes(app.id), app.name);
  assert.equal(resolved.releases.postgres.values.tls.enabled, true);
  assert.equal(resolved.releases.apeiron.values.image.backend.includes('YOUR_VERSION'), true);
  for (const name of ['kps', 'loki', 'promtail']) assert.equal(resolved.releases[name].enabled, false);
  const inventory = (await helmfile('build')).releases as Array<{ name: string; namespace: string; needs?: string[] }>;
  const enabled = new Set(inventory.map(r => r.namespace + '/' + r.name));
  for (const release of inventory) for (const need of release.needs ?? []) assert.equal(enabled.has(need), true, `${release.name} needs ${need}`);
  assert.equal(inventory.some(r => r.name === 'nexus'), true);
  assert.equal(inventory.some(r => r.name === 'langfuse'), false);
  assert.equal(inventory.some(r => r.name === 'kps'), false);
  console.log(`PASS real Helmfile print-env/build: ${inventory.length} releases, all dependencies present; no cluster connection or sync`);
} finally { await rm(dir, { recursive: true, force: true }); }
