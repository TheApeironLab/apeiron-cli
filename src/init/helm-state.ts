import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { join } from 'node:path';
import { ConfigError, type DeploymentTarget } from './config';

const execute = async (command: string, args: string[], env: NodeJS.ProcessEnv, signal: AbortSignal) =>
  (await promisify(execFile)(command, args, { env, signal, timeout: 20_000, maxBuffer: 262_144 })).stdout;

// Read Helm's current release metadata, never release values or Secret bodies.
// A pending release is not proof that its previous operation has stopped.
export async function checkHelmState(target: DeploymentTarget, env: NodeJS.ProcessEnv, signal: AbortSignal, run = execute): Promise<void> {
  let pending: { name: string; namespace: string; status: string }[];
  try {
    const args = ['list', '--pending', '--all-namespaces', '--output', 'json'];
    const output = target.runner === 'native'
      ? await run('helm', ['--kubeconfig', target.kubeconfig, ...args], env, signal)
      : await run('docker', ['run', '--rm', '--pull=never', '--name', env.LAB_CONTAINER!,
        '--network', env.LAB_NETWORK || `k3d-${env.LAB_CLUSTER || 'chentu-helmfile'}`,
        '--entrypoint', 'helm', '-v', `${join(target.workDir, 'state/kubeconfig')}:/kubeconfig:ro`, target.image,
        '--kubeconfig', '/kubeconfig', ...args], env, signal);
    const parsed: unknown = JSON.parse(output);
    if (!Array.isArray(parsed)) throw new Error();
    pending = parsed.map(item => {
      if (!item || !/^[a-z0-9][a-z0-9.-]{0,252}$/.test(item.name) || !/^[a-z0-9][a-z0-9-]{0,62}$/.test(item.namespace) ||
          !['pending-install', 'pending-upgrade', 'pending-rollback'].includes(item.status)) throw new Error();
      return { name: item.name, namespace: item.namespace, status: item.status };
    });
  } catch {
    signal.throwIfAborted();
    throw new ConfigError('无法确认 Helm 状态，尚未运行本次 sync。请检查集群连接和 Helm 读取权限后重试。');
  }
  if (pending.length) throw new ConfigError(`检测到未完成的 Helm 操作：${pending.slice(0, 12).map(item => `${item.namespace}/${item.name}（${item.status}）`).join('、')}。已阻止本次 sync；请先检查这些 release 的状态与历史，确认并处理上次操作后重新部署。不会自动删除记录或回滚。`);
}

// Only the ephemeral toolbox owned by this run may be removed. Confirm removal
// before releasing the deployment lock or allowing another attempt.
export async function removeToolbox(name: string): Promise<boolean> {
  try { await promisify(execFile)('docker', ['rm', '-f', name], { timeout: 15_000, maxBuffer: 4096 }); }
  catch { /* --rm may already have removed it. The following read is decisive. */ }
  try {
    const { stdout } = await promisify(execFile)('docker', ['ps', '-a', '--filter', `name=^/${name}$`, '--format', '{{.Names}}'], { timeout: 10_000, maxBuffer: 4096 });
    return stdout.trim() === '';
  } catch { return false; }
}
