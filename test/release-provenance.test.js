import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, writeFile, readFile, rm, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { sourceSnapshot, assertSourceUnchanged, buildProvenance, validateProvenance, inspectReleaseArtifact, sha256 } from '../scripts/lib/release-provenance';
const directories = [];
afterEach(async () => { await Promise.all(directories.splice(0).map(path => rm(path, { recursive: true, force: true }))); });
function git(root, ...args) {
  const child = Bun.spawnSync(['git', '-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', ...args], { cwd: root });
  if (child.exitCode) throw new Error('Fixture Git command failed');
}
async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'release-source-')); directories.push(root);
  await writeFile(join(root, 'Cargo.lock'), 'locked dependencies\n');
  await writeFile(join(root, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.98.0"\n');
  git(root, 'init', '-q'); git(root, 'add', '.'); git(root, 'commit', '-qm', 'source');
  const source = await sourceSnapshot(root);
  const binary = Buffer.from('real binary fixture');
  const provenance = buildProvenance(source, '0.2.0', 'darwin-arm64', binary, 'rustc 1.98.0 (exact build)', 'cargo 1.98.0 (exact build)');
  return { root, source, binary, provenance };
}
test('release source inspection rejects tracked changes and untracked build inputs', async () => {
  const { root } = await fixture();
  await writeFile(join(root, 'Cargo.lock'), 'altered dependencies');
  await expect(sourceSnapshot(root)).rejects.toThrow('clean committed');
  git(root, 'checkout', '--', 'Cargo.lock');
  await writeFile(join(root, 'uncommitted.rs'), 'untracked source');
  await expect(sourceSnapshot(root)).rejects.toThrow('clean committed');
});
test('a commit made during the build cannot label the old artifact with a new revision', async () => {
  const { root, source } = await fixture();
  await writeFile(join(root, 'Cargo.lock'), 'new committed dependencies');
  git(root, 'add', '.'); git(root, 'commit', '-qm', 'next');
  const after = await sourceSnapshot(root);
  expect(() => assertSourceUnchanged(source, after)).toThrow('Source changed');
});
test('provenance binds exact source tree, compiler, target and binary content', async () => {
  const { source, binary, provenance } = await fixture();
  validateProvenance(provenance, source, '0.2.0', 'darwin-arm64', binary);
  for (const change of [p => p.source.revision = 'f'.repeat(40), p => p.source.tree = 'f'.repeat(40),
    p => p.toolchain.rustc = 'rustc 1.97.0 (different)', p => p.target = 'linux-arm64', p => p.binary.sha256 = 'f'.repeat(64)]) {
    const altered = structuredClone(provenance); change(altered);
    expect(() => validateProvenance(altered, source, '0.2.0', 'darwin-arm64', binary)).toThrow();
  }
  expect(() => validateProvenance(provenance, source, '0.2.0', 'darwin-arm64', Buffer.from('tampered binary'))).toThrow('binary mismatch');
});
async function archiveFixture() {
  const f = await fixture();
  await writeFile(join(f.root, 'apeiron'), f.binary);
  await writeFile(join(f.root, 'README.md'), 'documentation');
  await writeFile(join(f.root, 'build-provenance.json'), JSON.stringify(f.provenance));
  const archive = join(f.root, 'artifact.tar.gz');
  const tar = () => { const result = Bun.spawnSync(['tar', '-czf', archive, '-C', f.root, 'apeiron', 'README.md', 'build-provenance.json']); if (result.exitCode) throw new Error('Fixture archive failed'); };
  const seal = async () => { const bytes = await readFile(archive); await writeFile(archive + '.provenance.json', JSON.stringify({ ...f.provenance, archive: { size: bytes.length, sha256: sha256(bytes) } })); };
  tar(); await seal(); return { ...f, archive, tar, seal };
}
test('finalization reads archive binary and rejects forged sidecars and symlinks', async () => {
  const f = await archiveFixture();
  expect(await inspectReleaseArtifact(f.archive, f.source, '0.2.0', 'darwin-arm64')).toEqual(f.provenance);
  const sidecar = JSON.parse(await readFile(f.archive + '.provenance.json', 'utf8'));
  sidecar.archive.sha256 = 'e'.repeat(64);
  await writeFile(f.archive + '.provenance.json', JSON.stringify(sidecar));
  await expect(inspectReleaseArtifact(f.archive, f.source, '0.2.0', 'darwin-arm64')).rejects.toThrow('Archive provenance checksum');
  await writeFile(join(f.root, 'apeiron'), 'modified binary'); f.tar(); await f.seal();
  await expect(inspectReleaseArtifact(f.archive, f.source, '0.2.0', 'darwin-arm64')).rejects.toThrow('binary mismatch');
  await rm(join(f.root, 'apeiron')); await symlink('README.md', join(f.root, 'apeiron')); f.tar(); await f.seal();
  await expect(inspectReleaseArtifact(f.archive, f.source, '0.2.0', 'darwin-arm64')).rejects.toThrow('regular files');
});
