import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { dirname, join } from 'node:path';
import { createConnection } from 'node:net';
import { ConfigError, type Configuration } from './config';
import type { Installation } from './installation';

const hash = (text: string) => createHash('sha256').update(text).digest('hex').slice(0, 12);
export function localCluster(configPath: string, domain: string) {
  const workDir = join(dirname(configPath), 'deployments', 'k3d-' + hash(configPath + '|' + domain));
  return { workDir, cluster: 'apeiron-' + hash(workDir) };
}

type Container = { cluster: string; running: boolean; bindings: Record<string, { HostIp?: string; HostPort: string }[] | null> };
export const inspectFormat = '{"cluster":{{json (index .Config.Labels "k3d.cluster")}},"bindings":{{json .HostConfig.PortBindings}},"running":{{json .State.Running}}}';
async function inspect(cluster: string, signal: AbortSignal): Promise<Container | null> {
  if (!Bun.which('docker')) throw new ConfigError('找不到 Docker，请先安装并启动 Docker，再开始部署。');
  let stdout: string;
  try {
    ({ stdout } = await promisify(execFile)('docker', ['inspect', '--format', inspectFormat, `k3d-${cluster}-serverlb`], { encoding: 'utf8', timeout: 10_000, signal }));
  } catch (error) {
    signal.throwIfAborted();
    const stderr = String((error as { stderr?: string }).stderr ?? '');
    if (/No such (?:object|container):/.test(stderr)) return null;
    throw new ConfigError('无法检查 Docker 中的集群，请确认 Docker 已启动后重试。');
  }
  try {
    const container = JSON.parse(stdout) as Container;
    if (!container || typeof container.running !== 'boolean' || !container.bindings || typeof container.bindings !== 'object') throw new Error();
    return container;
  } catch { throw new ConfigError('无法读取现有集群的端口配置，请检查 Docker 后重试。'); }
}

export type LocalPreflight = { mode: 'install' | 'redeploy'; message: string };
export async function checkLocalCluster(
  identity: { workDir: string; cluster: string }, installation: Pick<Installation, 'domain' | 'httpPort' | 'httpsPort'>,
  signal: AbortSignal, checks = { inspect, checkPort: checkLocalPort },
): Promise<LocalPreflight> {
  signal.throwIfAborted();
  const existing = await checks.inspect(identity.cluster, signal);
  if (existing) {
    let owned = false;
    try {
      const saved = JSON.parse(await readFile(join(identity.workDir, 'installation.json'), 'utf8'));
      owned = saved.cluster === identity.cluster && saved.domain === installation.domain;
    } catch { /* Missing or damaged records never authorize taking over a cluster. */ }
    if (!owned || existing.cluster !== identity.cluster) throw new ConfigError('同名集群不属于本次安装，请使用其他组织域名，避免接管其他集群。', 409);
    const bound = (containerPort: number, hostPort: number) => existing.bindings[`${containerPort}/tcp`]?.some(binding =>
      Number(binding.HostPort) === hostPort && ['127.0.0.1', '0.0.0.0', ''].includes(binding.HostIp ?? ''));
    if (!bound(installation.httpPort, installation.httpPort) || !bound(443, installation.httpsPort)) {
      throw new ConfigError('已找到本次安装的集群，但入口端口与当前配置不同。请填回创建集群时的端口，再重新部署。', 409);
    }
    if (!existing.running) throw new ConfigError('已找到本次安装的集群，但它已停止。请先启动该 K3d 集群，再重新部署。', 409);
    signal.throwIfAborted();
    return { mode: 'redeploy', message: '已识别本次安装的 K3d 集群，将复用集群重新部署，保留数据和凭据。' };
  }
  for (const port of [installation.httpPort, installation.httpsPort]) {
    signal.throwIfAborted();
    await checks.checkPort(port);
  }
  signal.throwIfAborted();
  return { mode: 'install', message: '入口端口可用，将创建新的 K3d 测试集群。' };
}

export async function preflightDeployment(config: Configuration, configPath: string, signal: AbortSignal): Promise<LocalPreflight | null> {
  const installation = config.deployment?.installation;
  if (installation?.topology !== 'single-k3d') return null;
  return checkLocalCluster(localCluster(configPath, installation.domain), installation, signal);
}

// Read-only; Docker still owns binding these ports during cluster creation.
export async function checkLocalPort(port: number): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const socket = createConnection({ host: '127.0.0.1', port });
    socket.setTimeout(1500);
    socket.once('connect', () => { socket.destroy(); reject(new ConfigError(`本机 ${port} 端口已被占用（其他集群或服务）。请返回部署环境修改入口端口。`, 409)); });
    socket.once('error', error => {
      socket.destroy();
      if ((error as NodeJS.ErrnoException).code === 'ECONNREFUSED') resolve();
      else reject(new ConfigError(`无法检查本机 ${port} 端口，请检查本机网络与 Docker。`));
    });
    socket.once('timeout', () => { socket.destroy(); reject(new ConfigError(`检查本机 ${port} 端口超时。`)); });
  });
}
