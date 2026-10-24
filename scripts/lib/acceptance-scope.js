import { parseReleaseVersion } from './release-version.js';

// `applications` is the pinned package's {ID: releases} (distribution-input.json).
export function acceptanceScope(version, applications) {
  if (Object.keys(applications ?? {}).length === 0 || !Object.values(applications).every(releases =>
      Array.isArray(releases) && releases.length > 0 && releases.every(name => typeof name === 'string')))
    throw new Error('Distribution must list its applications; pin a newer nightly');
  // The current nightly deferral does not reduce stable release coverage.
  const deferredApplications = parseReleaseVersion(version).channel === 'nightly' ? ['gpustack'] : [];
  const selected = Object.entries(applications).filter(([id]) => !deferredApplications.includes(id));
  return { applications: selected.map(([id]) => id).sort(), deferredApplications,
    releases: [...new Set(selected.flatMap(([, releases]) => releases))].sort() };
}

export function validateApplicationScope(applications, version, declared) {
  const expected = acceptanceScope(version, declared).applications;
  if (!Array.isArray(applications) || applications.length !== expected.length
      || new Set(applications).size !== applications.length
      || expected.some(name => !applications.includes(name)))
    throw new Error('Acceptance must cover every non-deferred wizard application exactly once');
}

export function workloadsReady(items) {
  return items.length > 0 && items.every(item => {
    const s = item.status || {};
    if ((s.observedGeneration || 0) < item.metadata.generation) return false;
    if (item.kind === 'DaemonSet')
      return s.desiredNumberScheduled > 0 && s.numberReady === s.desiredNumberScheduled
        && s.updatedNumberScheduled === s.desiredNumberScheduled && !s.numberUnavailable;
    const desired = item.spec.replicas ?? 1;
    return (s.readyReplicas || 0) === desired
      && (item.kind !== 'Deployment' || (s.updatedReplicas || 0) === desired && (s.availableReplicas || 0) === desired)
      && (item.kind !== 'StatefulSet' || s.currentRevision === s.updateRevision);
  });
}
