import { test } from 'bun:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink, lstat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

const installer = new URL('../scripts/install.sh', import.meta.url).pathname;
async function fixture(fn: (context: { directory: string; run: (...args: string[]) => ReturnType<typeof spawnSync>; shell: (script: string, env?: Record<string, string>) => ReturnType<typeof spawnSync>; archive: string; checksums: string; binary: string }) => Promise<void>) {
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
    const environment = {
      PATH: `${commands}:${directory}/home/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin`, HOME: join(directory, 'home'), SHELL: '/bin/zsh', FIXTURE: directory,
    };
    const run = (...args: string[]) => spawnSync('/bin/sh', [installer, ...args], { encoding: 'utf8', env: environment });
    const shell = (script: string, env: Record<string, string> = {}) => spawnSync('/bin/sh', ['-c', script, 'installer-test', installer], { encoding: 'utf8', env: { ...environment, ...env } });
    await fn({ directory, run, shell, archive, checksums, binary });
  } finally { await rm(directory, { recursive: true, force: true }); }
}

test('installer selects a standard bin already in PATH and works in the same parent shell', () => fixture(async ({ directory, shell, binary }) => {
  for (let i = 0; i < 2; i++) {
    const result = shell('/bin/sh "$1" && apeiron --version');
    assert.equal(result.status, 0, String(result.stderr));
    assert.match(String(result.stdout), /Ready in this terminal: apeiron init/);
    assert.match(String(result.stdout), /1\.2\.3\n$/);
  }
  assert.equal(spawnSync(binary, ['--version'], { encoding: 'utf8' }).stdout.trim(), '1.2.3');
  assert.equal(await Bun.file(join(directory, 'home/.zshrc')).exists(), false);
  assert.equal(await Bun.file(join(directory, 'commands/apeiron')).exists(), false);
}));

test('installer updates an existing command in place without changing its symlink target or shell hash', () => fixture(async ({ directory, shell, binary }) => {
  const bin = join(directory, 'home/.bun/bin'); await mkdir(bin, { recursive: true });
  const source = join(directory, 'original-command');
  await writeFile(source, '#!/bin/sh\necho old-version\n', { mode: 0o755 });
  await symlink(source, join(bin, 'apeiron'));
  const result = shell('apeiron --version; /bin/sh "$1" && apeiron --version', {
    PATH: `${directory}/commands:${directory}/home/.local/bin:${bin}:/usr/bin:/bin`,
  });
  assert.equal(result.status, 0, String(result.stderr));
  assert.match(String(result.stdout), /^old-version\n/);
  assert.match(String(result.stdout), /1\.2\.3\n$/);
  assert.equal((await lstat(join(bin, 'apeiron'))).isSymbolicLink(), false);
  assert.equal(await readFile(source, 'utf8'), '#!/bin/sh\necho old-version\n');
  assert.equal(await Bun.file(binary).exists(), false);
}));

test('installer refuses success when no supported bin exists in PATH', () => fixture(async ({ directory, shell, binary }) => {
  const result = shell('/bin/sh "$1"', { PATH: `${directory}/commands:/usr/bin:/bin` });
  assert.notEqual(result.status, 0);
  assert.match(String(result.stderr), /No supported bin directory/);
  assert.equal(await Bun.file(binary).exists(), false);
  assert.equal(await Bun.file(join(directory, 'home/.zshrc')).exists(), false);
}));

test('installer does not overwrite an unrelated command in a project or temporary PATH directory', () => fixture(async ({ directory, run, binary }) => {
  const command = join(directory, 'commands/apeiron');
  await writeFile(command, '#!/bin/sh\necho project-command\n', { mode: 0o755 });
  const result = run(); assert.notEqual(result.status, 0);
  assert.match(String(result.stderr), /takes precedence/);
  assert.equal(await readFile(command, 'utf8'), '#!/bin/sh\necho project-command\n');
  assert.equal(await Bun.file(binary).exists(), false);
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
