import { expect, test } from 'bun:test';
import { acceptanceScope, validateApplicationScope, workloadsReady } from '../scripts/lib/acceptance-scope.js';

test('GPUStack deferral is explicit for nightly and cannot reduce stable acceptance', () => {
  const nightly = '0.2.1-nightly.20261023.123';
  const scope = acceptanceScope(nightly);
  expect(scope.applications).not.toContain('gpustack');
  expect(scope.deferredApplications).toEqual(['gpustack']);
  validateApplicationScope(scope.applications, nightly);
  expect(() => validateApplicationScope(scope.applications, '0.2.1')).toThrow();
  expect(acceptanceScope('0.2.1').applications).toContain('gpustack');
  expect(acceptanceScope('0.2.1').deferredApplications).toEqual([]);
});

test('workload checks wait for observed rollout, ready replicas and current StatefulSet revision', () => {
  const deployment = { kind: 'Deployment', metadata: { generation: 2 }, spec: { replicas: 2 },
    status: { observedGeneration: 2, readyReplicas: 2, updatedReplicas: 2, availableReplicas: 2 } };
  const stateful = { kind: 'StatefulSet', metadata: { generation: 3 }, spec: { replicas: 1 },
    status: { observedGeneration: 3, readyReplicas: 1, currentRevision: 'new', updateRevision: 'new' } };
  const daemon = { kind: 'DaemonSet', metadata: { generation: 1 }, spec: {},
    status: { observedGeneration: 1, desiredNumberScheduled: 3, numberReady: 3, updatedNumberScheduled: 3 } };
  expect(workloadsReady([deployment, stateful, daemon])).toBe(true);
  expect(workloadsReady([])).toBe(false);
  for (const mutate of [xs => xs[0].status.observedGeneration = 1, xs => xs[0].status.updatedReplicas = 1,
    xs => xs[1].status.readyReplicas = 0, xs => xs[1].status.currentRevision = 'old',
    xs => xs[2].status.numberReady = 2]) {
    const items = structuredClone([deployment, stateful, daemon]); mutate(items);
    expect(workloadsReady(items)).toBe(false);
  }
  expect(workloadsReady([{ ...deployment, spec: { replicas: 0 }, status: { observedGeneration: 2 } }])).toBe(true);
});
