import { parseReleaseVersion } from './release-version.js';
import { validateApplicationScope, acceptanceScope } from './acceptance-scope.js';

// Deployment coverage is deliberately separate from the four executable targets.
export const acceptanceTargets = Object.freeze({
  'k3d-darwin-arm64': { cli: 'darwin-arm64', topology: 'single-k3d', os: 'darwin', arch: 'arm64', servers: 1 },
  'k3d-linux-arm64': { cli: 'linux-arm64', topology: 'single-k3d', os: 'linux', arch: 'arm64', servers: 1 },
  'k3s-single-amd64': { cli: 'linux-x64', topology: 'single-k3s', os: 'linux', arch: 'x64', servers: 1 },
  'k3s-multi-amd64': { cli: 'linux-x64', topology: 'multi-k3s', os: 'linux', arch: 'x64', servers: 3 },
});
export const acceptanceChecks = Object.freeze(['installation', 'nodes', 'images', 'credentials', 'https', 'login', 'applications', 'workloads']);
const requireValue = (value, message) => { if (!value) throw new Error(message); };
const sha = value => typeof value === 'string' && /^[a-f0-9]{40}$/.test(value);
const digest = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);

export function validateDistribution(input, release) {
  requireValue(input?.schemaVersion === 1, 'Unsupported distribution schema');
  parseReleaseVersion(input.version);
  requireValue(input.version === release.version, 'Distribution/CLI version mismatch');
  requireValue(sha(input.apeiron?.revision), 'Apeiron requires an exact commit');
  requireValue(input.apeiron.version === input.version, 'Apeiron build version mismatch');
  const images = input.apeiron.images;
  for (const component of ['backend', 'app', 'ops', 'scode']) {
    for (const arch of ['amd64', 'arm64']) {
      requireValue(/^[a-z0-9][a-z0-9.:-]*(?:\/[a-z0-9][a-z0-9._/-]*)?@sha256:[a-f0-9]{64}$/.test(images?.[component]?.[arch] || ''), `Missing immutable ${component}/${arch} image`);
    }
  }
  const chentu = input.chentu;
  requireValue(chentu?.version === input.version && sha(chentu.revision) && digest(chentu.sha256), 'Chentu requires matching version, commit and archive digest');
  requireValue(chentu.url === `https://downloads.apeironlab.cn/chentu/releases/${input.version}/chentu-${input.version}.tar.gz`, 'Unexpected Chentu archive URL');
  requireValue(sha(release.revision), 'CLI requires an exact commit');
  return input;
}

export function publicDistribution(input) {
  // Explicit projection: a local builder's credentials/configuration cannot
  // accidentally ride along when a distribution input is made public.
  return { schemaVersion: 1, version: input.version,
    apeiron: { version: input.apeiron.version, revision: input.apeiron.revision,
      images: Object.fromEntries(['backend', 'app', 'ops', 'scode'].map(name => [name,
        { amd64: input.apeiron.images[name].amd64, arm64: input.apeiron.images[name].arm64 }])) },
    chentu: { version: input.chentu.version, revision: input.chentu.revision, sha256: input.chentu.sha256, url: input.chentu.url } };
}

export function validateAcceptance(receipt, release, distributionHash, runId) {
  const target = acceptanceTargets[receipt?.target];
  requireValue(target, 'Unknown acceptance target');
  requireValue(receipt.schemaVersion === 1 && receipt.version === release.version && receipt.revision === release.revision,
    'Acceptance source/version mismatch');
  requireValue(receipt.distributionSha256 === distributionHash && digest(distributionHash), 'Acceptance distribution digest mismatch');
  requireValue(receipt.runId === runId && /^[1-9][0-9]*$/.test(runId), 'Acceptance must come from this workflow run');
  requireValue(receipt.archiveSha256 === release.files?.[`apeiron-${release.version}-${target.cli}.tar.gz`]?.sha256
    && digest(receipt.archiveSha256), 'Acceptance CLI archive digest mismatch');
  requireValue(receipt.os === target.os && receipt.arch === target.arch && receipt.topology === target.topology,
    'Acceptance host/topology mismatch');
  if (target.topology !== 'single-k3d') requireValue(receipt.osRelease === 'ubuntu:22.04', 'Native acceptance requires Ubuntu 22.04');
  requireValue(receipt.servers === target.servers && receipt.agents === 0, 'Acceptance node count mismatch');
  validateApplicationScope(receipt.applications, release.version);
  requireValue(JSON.stringify(receipt.deferredApplications) === JSON.stringify(acceptanceScope(release.version).deferredApplications), 'Acceptance must explicitly record deferred applications');
  requireValue(acceptanceChecks.every(name => receipt.checks?.[name] === true), 'Incomplete acceptance checks');
  return receipt;
}

export function validateAcceptanceSet(receipts, release, distributionHash, runId) {
  requireValue(Array.isArray(receipts) && receipts.length === Object.keys(acceptanceTargets).length, 'All four deployment targets must pass');
  const seen = new Set();
  for (const receipt of receipts) {
    validateAcceptance(receipt, release, distributionHash, runId);
    requireValue(!seen.has(receipt.target), 'Duplicate acceptance target');
    seen.add(receipt.target);
  }
}
