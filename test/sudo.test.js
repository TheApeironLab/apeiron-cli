import { test } from 'bun:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, readlink, realpath, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync, spawn } from 'node:child_process';
import { verifySudocodeArchive, prepareSudocode } from '../scripts/lib/sudocode-runtime.js';
import { createHash } from 'node:crypto';

const binary = resolve(process.env.APEIRON_TEST_BIN || 'target/debug/apeiron');

async function fixture(fn) {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-sudo-test-'));
  try {
    const home = join(root, 'home');
    const cwd = join(root, 'project with spaces');
    await mkdir(home); await mkdir(cwd);
    const fake = join(root, 'fake scode');
    await writeFile(fake, '#!/bin/sh\nprintf "cwd=%s\\nconfig=%s\\nupdates=%s\\n" "$PWD" "$SCODE_GLOBAL_CONFIG_DIR" "$SCODE_CHECK_UPDATES"\nprintf "arg=<%s>\\n" "$@"\ncat\necho diagnostic >&2\nexit 17\n', { mode: 0o755 });
    const env = { PATH: '/usr/bin:/bin', HOME: home, APEIRON_SCODE_BIN: fake };
    const run = (args, extra = {}) => spawnSync(binary, ['sudo', ...args], { cwd, env: { ...env, ...extra }, input: 'stdin-data\n', encoding: 'utf8' });
    await fn({ root, home, cwd, fake, env, run });
  } finally { await rm(root, { recursive: true, force: true }); }
}

test('sudo forwards arguments, working directory, input, stderr and exit status', () => fixture(async ({ home, cwd, run }) => {
  const result = run(['-p', 'spaces and $(not executed)', '--output-format', 'json', '--', '-literal']);
  assert.equal(result.status, 17, result.stderr);
  assert.ok(result.stdout.includes(`cwd=${await realpath(cwd)}\n`));
  assert.ok(result.stdout.includes(`config=${home}/.apeiron/sudo\nupdates=0\n`));
  assert.ok(result.stdout.includes('arg=<spaces and $(not executed)>\n'));
  assert.ok(result.stdout.endsWith('arg=<--output-format>\narg=<json>\narg=<-->\narg=<-literal>\nstdin-data\n'));
  assert.equal(result.stderr, 'diagnostic\n');
  const tools = join(home, '.apeiron/sudo/cli-tools/apeiron/tools');
  assert.equal(await readlink(join(tools, 'apeiron')), await realpath(binary));
  assert.match(await readFile(join(tools, 'APEIRON-CLI.md'), 'utf8'), /Model-provider credentials do not grant platform access/);
  assert.equal((await stat(tools)).mode & 0o777, 0o700);
}));

test('sudo respects explicit config root and does not replace model credentials', () => fixture(async ({ root, run }) => {
  const config = join(root, 'custom config'); await mkdir(config);
  const credentials = '{"existing":"do not modify"}\n';
  await writeFile(join(config, 'sudocode.json'), credentials, { mode: 0o600 });
  const result = run(['--model', 'chosen'], { SCODE_GLOBAL_CONFIG_DIR: config });
  assert.equal(result.status, 17, result.stderr);
  assert.ok(result.stdout.includes(`config=${config}\n`));
  assert.equal(await readFile(join(config, 'sudocode.json'), 'utf8'), credentials);
}));

test('sudo --help does not create configuration and stdout stays machine readable', () => fixture(async ({ home, fake, run }) => {
  await writeFile(fake, '#!/bin/sh\nprintf \'{"ok":true}\\n\'\n', { mode: 0o755 });
  const result = run(['--help']);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), { ok: true });
  assert.equal(await Bun.file(join(home, '.apeiron/sudo/cli-tools/apeiron/tools/APEIRON-CLI.md')).exists(), false);
}));

test('sudo preserves termination signals with no launcher process left behind', () => fixture(async ({ fake, env, cwd }) => {
  await writeFile(fake, '#!/bin/sh\necho ready\nexec /bin/sleep 30\n', { mode: 0o755 });
  const child = spawn(binary, ['sudo'], { cwd, env, stdio: ['ignore', 'pipe', 'pipe'] });
  const closed = new Promise((done, reject) => { child.once('error', reject); child.once('close', (code, signal) => done({ code, signal })); });
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
  try {
    await new Promise((done, reject) => { child.stdout.once('data', done); child.once('error', reject); child.once('exit', () => reject(new Error('Exited before ready'))); });
    child.kill('SIGTERM');
    assert.deepEqual(await closed, { code: null, signal: 'SIGTERM' });
  } finally { clearTimeout(timer); child.kill('SIGKILL'); }
}));

test('sudo rejects relative or recursive runtime overrides', () => fixture(async ({ run }) => {
  for (const override of ['scode', binary]) {
    const result = run([], { APEIRON_SCODE_BIN: override });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /absolute scode executable/);
  }
}));

test('runtime packaging rejects changed bytes or sizes', () => {
  const data = Buffer.from('fixture');
  const pin = { archiveSize: data.length, archiveSha256: createHash('sha256').update(data).digest('hex') };
  verifySudocodeArchive(data, pin);
  assert.throws(() => verifySudocodeArchive(Buffer.from('changed'), pin));
  assert.throws(() => verifySudocodeArchive(data, { ...pin, archiveSize: 1 }));
});

test('runtime packaging refuses independent Sudocode version drift before downloading', async () => {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-sudo-pins-'));
  try {
    await mkdir(join(root, 'release'));
    await writeFile(join(root, 'release/recipe.json'), JSON.stringify({ sudocode: { version: 'v1', commit: 'aaa' } }));
    await writeFile(join(root, 'release/sudocode-runtime.json'), JSON.stringify({ schemaVersion: 1, version: 'v2', commit: 'aaa' }));
    await assert.rejects(prepareSudocode(root, 'linux-x64', root), /must match/);
  } finally { await rm(root, { recursive: true, force: true }); }
});
