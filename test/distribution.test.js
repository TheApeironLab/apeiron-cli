import { acceptanceScope } from '../scripts/lib/acceptance-scope.js';
import { expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { acceptanceTargets, acceptanceChecks, validateDistribution, validateAcceptanceSet, publicDistribution } from '../scripts/lib/distribution.js';

const version = '0.2.1';
const hash = 'd'.repeat(64);
function fixture(version = '0.2.1') {
  const release = { version, revision: 'a'.repeat(40), files: {} };
  const input = { schemaVersion: 1, version, apeiron: { version, revision: 'b'.repeat(40), images: {} },
    chentu: { version, revision: 'c'.repeat(40), sha256: hash,
      url: `https://downloads.apeironlab.cn/chentu/releases/${version}/chentu-${version}.tar.gz` } };
  for (const name of ['backend', 'app', 'ops', 'scode']) input.apeiron.images[name] = { amd64: `registry.example/${name}@sha256:${hash}`, arm64: `registry.example/${name}@sha256:${hash}` };
  const receipts = Object.entries(acceptanceTargets).map(([target, t]) => {
    release.files[`apeiron-${version}-${t.cli}.tar.gz`] = { sha256: hash };
    return { schemaVersion: 1, version, revision: release.revision, target, distributionSha256: hash, archiveSha256: hash,
      runId: '123', os: t.os, arch: t.arch, topology: t.topology, osRelease: 'ubuntu:22.04', servers: t.servers, agents: 0,
      applications: acceptanceScope(version).applications, deferredApplications: acceptanceScope(version).deferredApplications,
      checks: Object.fromEntries(acceptanceChecks.map(name => [name, true])) };
  });
  return { release, input, receipts };
}

test('finalized public receipts retain application coverage and nightly deferrals', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-acceptance-finalize-'));
  try {
    const { release, input, receipts } = fixture('0.2.1-nightly.20261023.123');
    const inputBytes = JSON.stringify(input);
    const inputHash = createHash('sha256').update(inputBytes).digest('hex');
    const out = join(directory, 'release'), records = join(directory, 'receipts'), path = join(directory, 'input.json');
    await mkdir(out); await mkdir(records);
    await writeFile(path, inputBytes);
    await writeFile(join(out, 'release.json'), JSON.stringify(release));
    await writeFile(join(out, 'SHA256SUMS'), '');
    for (const receipt of receipts) {
      receipt.distributionSha256 = inputHash;
      receipt.privateProfile = 'must-not-be-published';
      await writeFile(join(records, receipt.target + '.json'), JSON.stringify(receipt));
    }
    const child = Bun.spawn([process.execPath, new URL('../scripts/finalize-distribution.js', import.meta.url).pathname,
      out, path, records], { env: { ...process.env, GITHUB_RUN_ID: '123' }, stdout: 'pipe', stderr: 'pipe' });
    expect(await child.exited).toBe(0);
    const text = await readFile(join(out, 'distribution.json'), 'utf8');
    const result = JSON.parse(text);
    validateAcceptanceSet(result.acceptance, release, inputHash, '123');
    expect(result.acceptance.every(r => r.deferredApplications.join() === 'gpustack')).toBe(true);
    expect(text).not.toContain('must-not-be-published');
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('one distribution binds all versions and requires images for both deployment architectures', () => {
  const { release, input } = fixture();
  validateDistribution(input, release);
  input.chentu.version = '0.1.0-rc.5';
  expect(() => validateDistribution(input, release)).toThrow('Chentu');
  input.chentu.version = version;
  delete input.apeiron.images.backend.arm64;
  expect(() => validateDistribution(input, release)).toThrow('backend/arm64');
});

test('single-node success cannot replace multi-node or either K3d acceptance', () => {
  const { release, receipts } = fixture();
  validateAcceptanceSet(receipts, release, hash, '123');
  expect(() => validateAcceptanceSet(receipts.slice(0, 3), release, hash, '123')).toThrow();
  receipts[3] = structuredClone(receipts[2]);
  expect(() => validateAcceptanceSet(receipts, release, hash, '123')).toThrow('Duplicate');
});

test('stale, mixed or incomplete receipts cannot authorize publication', () => {
  for (const mutate of [r => r.runId = '122', r => r.revision = 'e'.repeat(40), r => r.archiveSha256 = 'e'.repeat(64),
    r => r.distributionSha256 = 'e'.repeat(64), r => r.checks.login = false, r => r.servers = 1,
    r => r.osRelease = 'ubuntu:24.04', r => r.topology = 'single-k3s']) {
    const { release, receipts } = fixture();
    mutate(receipts[3]);
    expect(() => validateAcceptanceSet(receipts, release, hash, '123')).toThrow();
  }
});

test('a base-only or duplicated application receipt cannot authorize a full wizard release', () => {
  for (const applications of [undefined, ['nexus', 'vasi', 'apeiron', 'gateway'],
    [...acceptanceScope(version).applications, 'files'], acceptanceScope(version).applications.filter(name => name !== 'files')]) {
    const { release, receipts } = fixture();
    receipts[0].applications = applications;
    expect(() => validateAcceptanceSet(receipts, release, hash, '123')).toThrow('every non-deferred wizard application');
  }
});

test('public metadata excludes local builder settings and forbids credential-bearing image references', () => {
  const { release, input } = fixture();
  input.privateConfig = { apiKey: 'private-fixture' };
  input.apeiron.password = 'private-fixture';
  expect(JSON.stringify(publicDistribution(input))).not.toContain('private-fixture');
  input.apeiron.images.backend.amd64 = `user:password@registry.example/backend@sha256:${hash}`;
  expect(() => validateDistribution(input, release)).toThrow('backend/amd64');
});
