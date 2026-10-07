import { isIP } from 'node:net';
import { isAbsolute } from 'node:path';
import { ConfigError } from './config';

export type Topology = 'single-k3s' | 'single-k3d' | 'multi-k3s';
export interface NodeTarget {
  host: string;
  name: string;
  address: string;
  role: 'server' | 'agent';
}
export interface Installation {
  topology: Topology;
  domain: string;
  entryIp: string;
  httpPort: number;
  httpsPort: number;
  ha: boolean;
  sshUser: string;
  sshKey: string;
  sshPort: number;
  nodes: NodeTarget[];
  publicAccess?: PublicAccess;
}

export interface PublicAccess {
  mode: 'direct' | 'relay';
  publicIp: string;
  gateway?: { host: string; sshUser: string; sshKey: string; sshPort: number };
  tunnelPort: number;
}

export function validatePublicAccess(value: unknown): PublicAccess | undefined {
  if (value === undefined) return undefined;
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ConfigError('公网入口配置格式不正确。');
  const input = value as Record<string, unknown>;
  if (!['direct', 'relay'].includes(String(input.mode))) throw new ConfigError('请选择公网直连或 ECS 转发。');
  if (!safeEntryIp(input.publicIp) || /^(10|127|192\.168|172\.(1[6-9]|2\d|3[01])|100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7]))\./.test(input.publicIp)) throw new ConfigError('请填写公网入口 IPv4，不能使用局域网或 Tailscale 地址。');
  const tunnelPort = input.tunnelPort ?? 19444;
  if (!Number.isInteger(tunnelPort) || Number(tunnelPort) < 1024 || Number(tunnelPort) > 65535) throw new ConfigError('隧道端口需为 1024–65535。');
  let gateway: PublicAccess['gateway'];
  if (input.mode === 'relay') {
    const g = input.gateway as Record<string, unknown> | undefined;
    if (!g || !safeHost(g.host)) throw new ConfigError('请填写 ECS 的 SSH 地址或别名。');
    gateway = { host: g.host, ...validateConnection(g) };
  }
  return { mode: input.mode as PublicAccess['mode'], publicIp: input.publicIp, tunnelPort: Number(tunnelPort), ...(gateway ? { gateway } : {}) };
}

export const installationDefaults = (): Installation => ({
  topology: process.platform === 'darwin' ? 'single-k3d' : 'single-k3s',
  httpPort: process.platform === 'darwin' ? 54320 : 80, httpsPort: process.platform === 'darwin' ? 54321 : 443,
  domain: '', entryIp: process.platform === 'darwin' ? '127.0.0.1' : '', ha: false, sshUser: '', sshKey: '', sshPort: 22, nodes: [],
});
export const safeHost = (value: unknown): value is string => typeof value === 'string' &&
  value.length <= 253 && /^[a-zA-Z0-9][a-zA-Z0-9._-]*$/.test(value);
export const safeName = (value: unknown): value is string => typeof value === 'string' &&
  /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(value);
export const safeDomain = (value: unknown): value is string => typeof value === 'string' && value.length <= 220 &&
  value.includes('.') && value.split('.').every(safeName) && !isIP(value);
export const safeEntryIp = (value: unknown): value is string => typeof value === 'string' && isIP(value) === 4 &&
  !/^(0|169\.254|22[4-9]|23\d|24\d|25[0-5])\./.test(value) && value !== '255.255.255.255';

export function validateConnection(input: Record<string, unknown>) {
  const sshUser = input.sshUser ?? '';
  const sshKey = input.sshKey ?? '';
  const sshPort = input.sshPort ?? 22;
  if (typeof sshUser !== 'string' || (sshUser && !/^[a-z_][a-z0-9_-]{0,63}$/.test(sshUser))) throw new ConfigError('SSH 用户名格式不正确。');
  if (typeof sshKey !== 'string' || sshKey.length > 4096 || (sshKey && !isAbsolute(sshKey)) || /[\x00-\x1f\x7f{}]/.test(sshKey)) throw new ConfigError('SSH 私钥需填写本机绝对路径，或留空使用 SSH 配置／Agent。');
  if (!Number.isInteger(sshPort) || Number(sshPort) < 1 || Number(sshPort) > 65535) throw new ConfigError('SSH 端口必须在 1–65535 之间。');
  return { sshUser, sshKey, sshPort: Number(sshPort) };
}

export function validateInstallation(value: unknown): Installation {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ConfigError('请设置安装方式。');
  const input = value as Record<string, unknown>;
  if (!['single-k3s', 'single-k3d', 'multi-k3s'].includes(String(input.topology))) throw new ConfigError('请选择安装方式。');
  const topology = input.topology as Topology;
  const publicAccess = validatePublicAccess(input.publicAccess);
  if (publicAccess && topology !== 'single-k3s') throw new ConfigError('公网入口目前支持单机 K3s；K3d 和多机部署请使用内网访问。');
  if (!safeDomain(input.domain)) throw new ConfigError('请填写有效的平台域名，例如 team.apeironlab.internal。');
  if (publicAccess && /\.(internal|local|localhost|test|invalid|example)$/.test(input.domain)) throw new ConfigError('公网访问需要你持有的真实域名，不能使用 .internal 等内网域名。');
  // Older saved configurations did not contain an entry IP. Keep them readable;
  // the wizard and installation preflight require it before creating a cluster.
  const entryIp = input.entryIp ?? (topology === 'single-k3d' ? '127.0.0.1' : '');
  if (entryIp !== '' && (!safeEntryIp(entryIp) || (topology !== 'single-k3d' && entryIp.startsWith('127.')))) throw new ConfigError('请填写可访问的入口 IPv4 地址。');
  if (topology === 'single-k3d' && entryIp !== '127.0.0.1') throw new ConfigError('本机 K3d 的入口 IP 固定为 127.0.0.1。');
  if (typeof input.ha !== 'boolean') throw new ConfigError('高可用选项格式不正确。');
  if (input.ha && topology !== 'multi-k3s') throw new ConfigError('高可用仅适用于多机 K3s。');
  const httpPort = input.httpPort ?? (topology === 'single-k3d' ? 54320 : 80);
  const httpsPort = input.httpsPort ?? (topology === 'single-k3d' ? 54321 : 443);
  if (![httpPort, httpsPort].every(port => Number.isInteger(port) && Number(port) >= 1 && Number(port) <= 65535) || httpPort === httpsPort) throw new ConfigError('HTTP / HTTPS 端口需为 1–65535，且不能相同。');
  if (topology !== 'single-k3d' && (httpPort !== 80 || httpsPort !== 443)) throw new ConfigError('K3s 使用标准 HTTP 80 / HTTPS 443 端口。');
  if (topology === 'single-k3d' && (httpPort === 443 || httpsPort === 80)) throw new ConfigError('HTTP 不能使用 443，HTTPS 不能使用 80。');
  const connection = validateConnection(input);
  if (!Array.isArray(input.nodes) || input.nodes.length > 32) throw new ConfigError('最多支持 32 个节点。');
  const nodes: NodeTarget[] = input.nodes.map(node => {
    if (!node || typeof node !== 'object' || !safeHost(node.host) || !safeName(node.name) ||
        typeof node.address !== 'string' || isIP(node.address) !== 4 || !['server', 'agent'].includes(node.role)) throw new ConfigError('请检测节点，并确认节点名称、内网 IPv4 和角色。');
    return { host: node.host, name: node.name, address: node.address, role: node.role };
  });
  for (const key of ['host', 'name', 'address'] as const) if (new Set(nodes.map(node => node[key])).size !== nodes.length) throw new ConfigError('节点地址和名称不能重复。');
  if (topology === 'multi-k3s') {
    if (nodes.length < 2) throw new ConfigError('多机部署至少需要 2 台机器；1 个控制节点 + 1 个工作节点即可。');
    const servers = nodes.filter(node => node.role === 'server').length;
    if (input.ha ? servers < 3 || servers % 2 === 0 : servers !== 1) throw new ConfigError(input.ha ? '高可用需要 3 个或更多奇数个控制节点。' : '普通多机部署需要 1 个控制节点，其余为工作节点。');
  } else if (nodes.length) throw new ConfigError('单机部署不接受远程节点列表。');
  return { topology, domain: input.domain, entryIp, httpPort: Number(httpPort), httpsPort: Number(httpsPort), ha: input.ha, ...connection, nodes, ...(publicAccess ? { publicAccess } : {}) };
}

export function inventoryFor(installation: Installation, nodes: NodeTarget[], bundle: string, kubeconfig: string, architecture: 'amd64' | 'arm64') {
  const hosts = (role: NodeTarget['role']) => Object.fromEntries(nodes.filter(node => node.role === role).map(node => [node.name, {
    ansible_host: node.host, chentu_node_ip: node.address,
    ...(installation.topology === 'single-k3s' ? { ansible_connection: 'local' } : {}),
  }]));
  return { all: { vars: {
    ansible_become: true, ansible_ssh_common_args: '-o BatchMode=yes -o StrictHostKeyChecking=yes -o ConnectTimeout=8',
    ...(installation.sshUser ? { ansible_user: installation.sshUser } : {}),
    ...(installation.sshKey ? { ansible_ssh_private_key_file: installation.sshKey } : {}),
    ansible_port: installation.sshPort, chentu_domain: installation.domain, chentu_architecture: architecture,
    chentu_bundle: bundle, chentu_remote_repo: '/opt/chentu/setup', chentu_kubeconfig: kubeconfig,
  }, children: { server: { hosts: hosts('server') }, agent: { hosts: hosts('agent') } } } };
}
