import { readFileSync } from 'node:fs';

const catalog = JSON.parse(readFileSync(new URL('../../runtime/assets/apps.json', import.meta.url)));
import { parseReleaseVersion } from './release-version.js';

export function acceptanceScope(version) {
  // The current nightly deferral does not reduce stable release coverage.
  const deferredApplications = parseReleaseVersion(version).channel === 'nightly' ? ['gpustack'] : [];
  const selected = catalog.filter(app => !deferredApplications.includes(app.id));
  return { applications: selected.map(app => app.id).sort(), deferredApplications,
    releases: [...new Set(selected.flatMap(app => app.releases || [app.id]))].sort() };
}

export function validateApplicationScope(applications, version) {
  const expected = acceptanceScope(version).applications;
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
