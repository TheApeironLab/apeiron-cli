import { test, expect } from 'bun:test';
import { cleanupK3dLease } from '../scripts/lib/k3d-lease.js';

test('K3d lease cleanup includes the detached registry and only uses exact owned selectors', async () => {
  const calls = [];
  const cluster = 'apeiron-123456abcdef';
  await cleanupK3dLease(cluster, async args => {
    calls.push(args);
    if (args[0] === 'ps') {
      expect(args).toContain('label=app=k3d');
      if (args.includes('label=k3d.role=registry')) {
        expect(args).toContain(`name=^/k3d-${cluster}-reg$`);
        return ['registry'];
      }
      expect(args).toContain(`label=k3d.cluster=${cluster}`);
      return ['node', 'tools'];
    }
    if (args[1] === 'ls') {
      expect(args).toContain('label=app=k3d');
      if (args[0] === 'network') {
        expect(args).toContain(`name=^k3d-${cluster}$`);
        return ['network'];
      }
      expect(args).toContain(`label=k3d.cluster=${cluster}`);
      return ['images-volume'];
    }
    return [];
  });
  expect(calls.filter(x => x.includes('rm'))).toEqual([
    ['rm', '-f', '-v', 'node', 'tools', 'registry'], ['network', 'rm', 'network'], ['volume', 'rm', 'images-volume'],
  ]);
  let touched = false;
  await expect(cleanupK3dLease('business-cluster', async () => { touched = true; })).rejects.toThrow('identity');
  expect(touched).toBe(false);
});
