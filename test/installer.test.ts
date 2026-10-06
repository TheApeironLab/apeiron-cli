import { test } from 'bun:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

const installer = new URL('../scripts/install.sh', import.meta.url).pathname;
async function fixture(fn: (context: { directory: string; run: (...args: string[]) => ReturnType<typeof spawnSync>; archive: string; checksums: string; binary: string }) => Promise<void>) {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-installer-test-'));
  try {
    for (const name of ['commands', 'source', 'home']) await mkdir(join(directory, name));
    const commands = join(directory, 'commands');
    const binary = join(directory, 'home/.local/bin/apeiron');
    const executable = async (name: string, text: string) => writeFile(join(commands, name), `#!/bin/sh\n${text}\n`, { mode: 0o755 });
    await executable('uname', 'if [ "$1" = -s ]; then echo Linux; else echo aarch64; fi');
    // Simulate an HTTPS download while retaining the real archive/checksum/install operations.
    await executable('curl', 'while [ "$#" -gt 0 ]; do case "$1" in --output) output=$2; shift 2;; https://*) url=$1; shift;; *) shift;; esac; done\ncp "$FIXTURE/${url##*/}" "$output"');
    const archive = join(directory, 'apeiron-1.2.3-linux-arm64.tar.gz');
    const checksums = join(directory, 'SHA256SUMS');
    await writeFile(join(directory, 'source/apeiron'), '#!/bin/sh\necho 1.2.3\n', { mode: 0o755 });
    assert.equal(spawnSync('tar', ['-czf', archive, '-C', join(directory, 'source'), 'apeiron']).status, 0);
    const digest = createHash('sha256').update(await readFile(archive)).digest('hex');
    await writeFile(checksums, `${digest}  apeiron-1.2.3-linux-arm64.tar.gz\n`);
    await writeFile(join(directory, 'latest.txt'), '1.2.3\n');
    const run = (...args: string[]) => spawnSync('/bin/sh', [installer, ...args], { encoding: 'utf8', env: {
      PATH: `${commands}:/usr/bin:/bin:/usr/sbin:/sbin`, HOME: join(directory, 'home'), SHELL: '/bin/zsh', FIXTURE: directory,
    } });
    await fn({ directory, run, archive, checksums, binary });
  } finally { await rm(directory, { recursive: true, force: true }); }
}

test('installer selects native archive, installs without sudo, and updates PATH once', () => fixture(async ({ directory, run, binary }) => {
  for (let i = 0; i < 2; i++) {
    const result = run(); assert.equal(result.status, 0, String(result.stderr));
  }
  assert.equal(spawnSync(binary, ['--version'], { encoding: 'utf8' }).stdout.trim(), '1.2.3');
  const profile = await readFile(join(directory, 'home/.zshrc'), 'utf8');
  assert.equal(profile.split('# Apeiron CLI PATH').length, 2);
}));

test('installer rejects corrupt downloads without replacing an existing installation', () => fixture(async ({ run, archive, binary }) => {
  assert.equal(run().status, 0);
  await writeFile(binary, 'previous-installation');
  await writeFile(archive, 'corrupt archive');
  const result = run(); assert.notEqual(result.status, 0); assert.match(String(result.stderr), /Checksum mismatch/);
  assert.equal(await readFile(binary, 'utf8'), 'previous-installation');
}));

test('installer rejects duplicate checksum records and invalid versions', () => fixture(async ({ run, checksums }) => {
  await writeFile(checksums, (await readFile(checksums, 'utf8')).repeat(2));
  const duplicate = run(); assert.notEqual(duplicate.status, 0); assert.match(String(duplicate.stderr), /duplicated/);
  const invalid = run('--version', '../unexpected'); assert.notEqual(invalid.status, 0); assert.match(String(invalid.stderr), /Invalid release version/);
}));

test('custom install directory works without modifying the shell profile', () => fixture(async ({ directory, run }) => {
  const custom = join(directory, 'custom directory');
  const result = run('--install-dir', custom, '--no-modify-path');
  assert.equal(result.status, 0, String(result.stderr));
  assert.equal(spawnSync(join(custom, 'apeiron'), ['--version'], { encoding: 'utf8' }).stdout.trim(), '1.2.3');
  assert.equal(await Bun.file(join(directory, 'home/.zshrc')).exists(), false);
  assert.doesNotMatch(String(result.stdout), /In a new terminal/);
}));
