import { expect, test } from 'bun:test';
import { checkHelmState } from '../src/init/helm-state';
import { deploymentDefaults } from '../src/init/config';

test('Helm preflight pins the deployment kubeconfig and rejects pending releases and unreadable results', async () => {
  const target = { ...deploymentDefaults(), runner: 'native' as const, kubeconfig: '/fixture/kubeconfig' };
  const signal = new AbortController().signal;
  await checkHelmState(target, {}, signal, async (command, args) => {
    expect(command).toBe('helm');
    expect(args).toEqual(['--kubeconfig', '/fixture/kubeconfig', 'list', '--pending', '--all-namespaces', '--output', 'json']);
    return '[]';
  });
  for (const status of ['pending-install', 'pending-upgrade', 'pending-rollback']) {
    await expect(checkHelmState(target, {}, signal, async () => JSON.stringify([{ name: 'apeiron', namespace: 'apeiron', status }]))).rejects.toThrow(`apeiron/apeiron（${status}）`);
  }
  for (const output of ['', '{}', '[{"name":"bad data"}]']) await expect(checkHelmState(target, {}, signal, async () => output)).rejects.toThrow('无法确认 Helm 状态');
  await expect(checkHelmState(target, {}, signal, async () => { throw new Error('private connection data'); })).rejects.toThrow('无法确认 Helm 状态');
  await checkHelmState({ ...target, runner: 'docker', workDir: '/fixture/work', image: 'fixture-image' }, { LAB_CONTAINER: 'owned-toolbox', LAB_CLUSTER: 'owned-cluster' }, signal, async (command, args) => {
    expect(command).toBe('docker'); expect(args).toContain('--pull=never'); expect(args).toContain('owned-toolbox');
    expect(args).toContain('k3d-owned-cluster'); expect(args).toContain('/fixture/work/state/kubeconfig:/kubeconfig:ro');
    expect(args.join(' ')).not.toContain('docker.sock');
    return '[]';
  });
});
