// secrets: none. Observes the real Rust executable; never launches a deployment.
import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { binary } from './support';

const dirs = [];
afterEach(async () => { for (const dir of dirs.splice(0)) await rm(dir, { recursive: true, force: true }); });
const cli = (...args) => {
  const process = Bun.spawnSync([binary, 'operation', ...args]);
  return { code: process.exitCode, stdout: process.stdout.toString(), stderr: process.stderr.toString() };
};

test('operation discovery documents read-only observation with an explicit target', () => {
  const help = cli('--help');
  expect(help.code).toBe(0);
  expect(help.stdout).toContain('status|events');
  expect(help.stdout).toContain('Legacy checkpoints are not adopted');
  expect(cli('status').code).toBe(2);
  expect(cli('status', '--config', '/unused', '--after', '1').code).toBe(2);
  expect(cli('events', '--config', '/unused', '--after', '-1').code).toBe(2);
});

test('unsupported installation observation neither creates a lease nor adopts a checkpoint', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-operation-')); dirs.push(dir);
  const path = join(dir, 'config.json');
  await writeFile(path, '{}', { mode: 0o600 });
  for (const action of ['status', 'events']) {
    const result = cli(action, '--config', path, '--json');
    expect(result.code).toBe(2);
    expect(result.stderr).toContain('UNSUPPORTED_OPERATION_CHECKPOINT');
    expect(result.stdout).toBe('');
  }
  expect(await readdir(dir)).toEqual(['config.json']);
});
