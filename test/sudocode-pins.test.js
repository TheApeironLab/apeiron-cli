import { test } from 'bun:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { pinSudocode } from '../scripts/pin-sudocode.js';
import { prepareSudocode, runtimeUrl, sudocodeSource } from '../scripts/lib/sudocode-runtime.js';
import { sha256 } from '../scripts/lib/release-provenance.js';

async function fixture(fn) {
  const temporary = await mkdtemp(join(tmpdir(), 'apeiron-sudocode-pins-'));
  try {
    const root = join(temporary, 'host');
    const source = join(root, 'third_party/sudocode');
    const archives = join(temporary, 'archives');
    await mkdir(join(source, 'rust'), { recursive: true });
    await mkdir(join(root, 'release'));
    await mkdir(archives);
    const git = args => execFileSync('git', ['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', ...args], { cwd: source, encoding: 'utf8' }).trim();
    git(['init', '--quiet']);
    await writeFile(join(source, 'rust/Cargo.toml'), '[workspace.package]\nversion = "1.2.3"\n');
    git(['add', '.']); git(['commit', '--quiet', '-m', 'Fixture source']); git(['tag', 'v1.2.3']);
    await writeFile(join(root, 'release/recipe.json'), JSON.stringify({ unrelated: 'preserve', sudocode: {} }));
    const sums = [];
    for (const target of ['macos-arm64', 'macos-x64', 'linux-arm64', 'linux-x64']) {
      const name = 'scode-' + target;
      await mkdir(join(archives, name));
      await writeFile(join(archives, name, 'scode'), 'binary fixture ' + target, { mode: 0o755 });
      execFileSync('tar', ['-czf', join(archives, name + '.tar.gz'), '-C', archives, name]);
      sums.push(`${sha256(await readFile(join(archives, name + '.tar.gz')))}  ${name}.tar.gz`);
    }
    await writeFile(join(archives, 'SHA256SUMS.txt'), sums.join('\n') + '\n');
    await fn({ root, source, archives, git });
  } finally { await rm(temporary, { recursive: true, force: true }); }
}

test('Sudocode pins are generated from the submodule and support HTTPS mirrors and local packaging', () => fixture(async ({ root, archives, git }) => {
  const identity = await pinSudocode(root, archives, 'https://mirror.example.internal/scode/v1.2.3');
  assert.deepEqual(identity, { version: 'v1.2.3', commit: git(['rev-parse', 'HEAD']) });
  const path = join(root, 'release/sudocode-runtime.json');
  const before = await readFile(path, 'utf8');
  const manifest = JSON.parse(before);
  assert.equal(Object.keys(manifest.platforms).length, 4);
  assert.equal(manifest.platforms['linux-x64'].url, 'https://mirror.example.internal/scode/v1.2.3/scode-linux-x64.tar.gz');
  assert.equal(manifest.platforms['linux-x64'].binarySha256, sha256(Buffer.from('binary fixture linux-x64')));
  assert.equal(JSON.parse(await readFile(join(root, 'release/recipe.json'), 'utf8')).unrelated, 'preserve');
  await pinSudocode(root, archives, 'https://mirror.example.internal/scode/v1.2.3');
  assert.equal(await readFile(path, 'utf8'), before);
  const previous = process.env.APEIRON_SCODE_ARCHIVE_DIR;
  process.env.APEIRON_SCODE_ARCHIVE_DIR = archives;
  try {
    const prepared = await prepareSudocode(root, 'linux-x64', archives);
    assert.equal(sha256(await readFile(prepared.path)), manifest.platforms['linux-x64'].archiveSha256);
    manifest.commit = '0'.repeat(40);
    await writeFile(path, JSON.stringify(manifest));
    await writeFile(join(root, 'release/recipe.json'), JSON.stringify({ sudocode: { ...identity, commit: manifest.commit } }));
    await assert.rejects(prepareSudocode(root, 'linux-x64', archives), /must match the submodule/);
  } finally {
    if (previous === undefined) delete process.env.APEIRON_SCODE_ARCHIVE_DIR;
    else process.env.APEIRON_SCODE_ARCHIVE_DIR = previous;
  }
}));

test('Sudocode pin generation rejects changed archives before writing configuration', () => fixture(async ({ root, archives }) => {
  const recipe = await readFile(join(root, 'release/recipe.json'), 'utf8');
  await writeFile(join(archives, 'scode-linux-x64.tar.gz'), 'corrupted');
  await assert.rejects(pinSudocode(root, archives, 'https://mirror.example.internal/release/'), /reviewed runtime pin/);
  assert.equal(await readFile(join(root, 'release/recipe.json'), 'utf8'), recipe);
  await assert.rejects(readFile(join(root, 'release/sudocode-runtime.json')), { code: 'ENOENT' });
}));

test('Sudocode prebuilt pins require a clean source and its matching release tag', () => fixture(async ({ root, source, archives, git }) => {
  await writeFile(join(source, 'untracked'), 'dirty');
  await assert.rejects(sudocodeSource(root), /must be clean/);
  git(['add', '.']); git(['commit', '--quiet', '-m', 'Unreleased change']);
  await assert.rejects(pinSudocode(root, archives, 'https://mirror.example.internal/release/'), /matching release tag/);
}));

test('Sudocode download source accepts HTTPS mirrors but rejects insecure or credential-bearing URLs', () => {
  assert.equal(runtimeUrl('https://oss.example.internal/blob/abc').hostname, 'oss.example.internal');
  for (const value of ['http://example.internal/file', 'https://user:secret@example.internal/file', 'https://example.internal/file?token=secret'])
    assert.throws(() => runtimeUrl(value), /credential-free HTTPS/);
});
