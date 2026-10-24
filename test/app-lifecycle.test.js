import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { binary } from './support';
const dirs = [];
afterEach(async () => { for (const dir of dirs.splice(0)) await rm(dir, { recursive: true, force: true }); });
const cli = (...args) => {
  const p = Bun.spawnSync([binary, 'app', ...args]);
  return { code: p.exitCode, stdout: p.stdout.toString(), stderr: p.stderr.toString() };
};
async function fixture() {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-app-')); dirs.push(dir);
  const path = join(dir, 'config.json');
  const bytes = JSON.stringify({ schemaVersion: 2, slug: 'example-team', apps: ['nexus', 'vasi', 'ontology', 'apeiron', 'zygos', 'gateway', 'git', 'ci'] });
  await writeFile(path, bytes, { mode: 0o600 });
  return { dir, path, bytes };
}
test('app help documents retained data and requires an explicit target', () => {
  expect(cli('--help').code).toBe(0);
  expect(cli('--help').stdout).toContain('retain PVCs');
  expect(cli('install', 'zygos').stderr).toContain('--instance or --config');
  expect(cli('install', 'zygos', '--instance', 'team', '--config', '/tmp/unused').code).toBe(2);
});
test('app list and requests read the installed package catalog and change nothing without an installation', async () => {
  // Protected, dependency and unknown-ID rules against a catalog: applications.rs unit tests.
  const f = await fixture();
  expect(cli('uninstall', 'zygos', '--config', f.path).stderr).toContain('--yes');
  for (const args of [['list', '--json'], ['uninstall', 'nexus', '--yes'], ['update', 'task'], ['install', 'unknown']])
    expect(cli(...args, '--config', f.path).stderr).toContain('No completed installation');
  expect(await readFile(f.path, 'utf8')).toBe(f.bytes);
  expect((await readdir(f.dir)).filter(n => n.includes('resume') || n === 'deployments')).toEqual([]);
});
test('an uninstalled or unbound configuration cannot be adopted by app install or dry-run', async () => {
  const f = await fixture();
  for (const extra of [[], ['--dry-run']]) {
    const result = cli('install', 'zygos', '--config', f.path, ...extra);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toContain('No completed installation');
    expect(await readFile(f.path, 'utf8')).toBe(f.bytes);
  }
});
