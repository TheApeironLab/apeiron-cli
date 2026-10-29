import { test, expect } from 'bun:test';
import { binary } from './support';
import { readFile } from 'node:fs/promises';
const run = args => Bun.spawnSync([binary, ...args], { stdout: 'pipe', stderr: 'pipe' });
test('Chentu adapter shares commands, failures and selected distribution with legacy entries', async () => {
  for (const args of [['init', '--help'], ['app', '--help'], ['app', '--invalid'], ['platform', 'entry', '--help']]) {
    const embedded = run(['chentu', ...args]);
    const legacy = run(args);
    expect(embedded.exitCode).toBe(legacy.exitCode);
    expect(embedded.stdout.toString()).toBe(legacy.stdout.toString());
    expect(embedded.stderr.toString()).toBe(legacy.stderr.toString());
  }
  expect(run(['chentu', 'unknown']).exitCode).toBe(2);
  const manifest = JSON.parse(await readFile(new URL('../release/distribution-input.json', import.meta.url), 'utf8'));
  const info = JSON.parse(run(['chentu', '--build-info']).stdout);
  expect(info.deployment).toEqual(manifest.chentu);
  expect(JSON.parse(run(['--build-info']).stdout).chentu).toEqual(info);
});
