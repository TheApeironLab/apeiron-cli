import { execFile } from 'node:child_process';
import { mkdir, readdir, readFile, lstat } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { ConfigError, ConfigStore } from './config';
import { resolveChentu, type ResourceResolver } from '../resources/chentu';
import { safeDomain } from './installation';
import { checkDns } from './dns';
import { resolve4, resolve6 } from 'node:dns/promises';

export interface Connection {
  id: string; identity: string; domain: string; host: string; sshPort: number;
  publicIp: string; tunnelPort: number; state: 'paired' | 'revoked';
  tunnel?: boolean; routes?: boolean; checkedAt?: number;
  dns?: boolean; https?: boolean; message?: string;
  localStopped?: boolean;
}
export const connectionDirectory = (config: string) => join(dirname(config), 'connections');
const validId = (id: unknown): id is string => typeof id === 'string' && /^[a-f0-9]{32}$/.test(id);

// Subprocess input contains the invitation: stdin only, never argv/env/logs.
export async function pairingHelper(root: string, args: string[], input: object, signal: AbortSignal): Promise<Record<string, unknown>> {
  const script = join(root, 'bootstrap/public_pairing.py');
  if (!await Bun.file(script).exists()) throw new ConfigError('此宸途发行包尚未包含配对与连接管理，请使用新版安装包。');
  return new Promise((resolve, reject) => {
    const child = execFile('python3', [script, ...args], { signal, timeout: 60_000, maxBuffer: 65_536 }, (error, stdout) => {
      if (error) { reject(new ConfigError('入口操作未确认成功。请检查配对码是否过期、SSH 是否可达，或在 ECS 运行 apeiron platform entry status。', 502)); return; }
      try { resolve(JSON.parse(stdout)); } catch { reject(new ConfigError('入口返回了无效响应。', 502)); }
    });
    child.stdin?.on('error', () => {});
    child.stdin?.end(JSON.stringify(input));
  });
}

export class PairingManager {
  readonly directory: string;
  constructor(config: string, private readonly resources: ResourceResolver = resolveChentu) { this.directory = connectionDirectory(config); }
  async list(): Promise<Connection[]> {
    const entries = await readdir(this.directory).catch((error: NodeJS.ErrnoException) => { if (error.code === 'ENOENT') return []; throw error; });
    const connections = await Promise.all(entries.filter(validId).map(id => this.get(id).catch((error: NodeJS.ErrnoException) => { if (error.code === 'ENOENT') return undefined; throw error; })));
    return connections.filter((connection): connection is Connection => connection !== undefined);
  }
  async get(id: string): Promise<Connection> {
    if (!validId(id)) throw new ConfigError('连接 ID 格式不正确。');
    const directory = join(this.directory, id);
    if ((await lstat(directory)).isSymbolicLink()) throw new ConfigError('连接目录不能是符号链接。');
    const file = join(directory, 'connection.json');
    const stat = await lstat(file);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 16_384) throw new ConfigError('连接记录格式不正确。');
    const value = JSON.parse(await readFile(file, 'utf8'));
    if (value.id !== id || !safeDomain(value.domain) || !/^apeiron-[a-f0-9]{12}$/.test(value.identity)) throw new ConfigError('连接记录无效。');
    // Explicit projection: private keys, invitation and host-key material never reach the UI.
    return { id, identity: value.identity, domain: value.domain, host: value.host, sshPort: value.sshPort,
      publicIp: value.publicIp, tunnelPort: value.tunnelPort, state: value.state === 'revoked' ? 'revoked' : 'paired' };
  }
  private async run(action: string, id: string | undefined, input: object, signal: AbortSignal) {
    const root = await this.resources('', signal, () => {}, { offline: false, bundleDir: '' });
    return pairingHelper(root, [action, this.directory, ...(id ? [id] : [])], input, signal);
  }
  async pair(code: string, signal: AbortSignal): Promise<Connection> {
    if (typeof code !== 'string' || code.length > 8192 || !/^apeiron-pair-v1\.[A-Za-z0-9_-]+$/.test(code)) throw new ConfigError('请粘贴 ECS 生成的完整配对码。');
    await new ConfigStore(join(this.directory, 'guard')).checkLocation();
    await mkdir(this.directory, { recursive: true, mode: 0o700 });
    const result = await this.run('client-pair', undefined, { code }, signal);
    if (!validId(result.id)) throw new ConfigError('配对结果无效。');
    return this.get(result.id);
  }
  async scope(id: string, domain: string, publicIp: string, tunnelPort: number) {
    const connection = await this.get(id);
    if (connection.state !== 'paired' || connection.domain !== domain || connection.publicIp !== publicIp || connection.tunnelPort !== tunnelPort) throw new ConfigError('配对入口已撤销或与当前平台域名不匹配，请重新配对或选择对应域名。');
    return connection;
  }
  async action(action: 'status' | 'test' | 'revoke', id: string, signal: AbortSignal): Promise<Connection> {
    const connection = await this.get(id);
    if (connection.state === 'revoked') return { ...connection, tunnel: false, routes: false, message: '此连接已撤销，需要在 ECS 重新生成配对码。' };
    const result = await this.run(action === 'revoke' ? 'client-revoke' : 'client-status', id, {}, signal);
    if (result.id !== id || !['paired', 'revoked'].includes(String(result.state))) throw new ConfigError('入口状态无效。');
    const status: Connection = { ...connection, state: result.state as Connection['state'],
      tunnel: result.tunnel === true, routes: result.routes === true, checkedAt: Date.now() / 1000 };
    if (action === 'revoke') status.localStopped = result.localStopped !== false;
    if (action !== 'test' || status.state === 'revoked') return status;
    const dns = await checkDns({ domain: connection.domain, entryIp: connection.publicIp, local: false }, {
      signal, includeRoot: true,
      lookup: async host => (await Promise.all([resolve4(host).catch(() => []), resolve6(host).catch(() => [])])).flat().map(address => ({ address })),
    });
    const https = await Promise.all(['apeiron', 'iam'].map(async name => {
      const host = `${name}.${connection.domain}`;
      return new Promise<boolean>(resolve => {
        execFile('curl', ['--noproxy', '*', '--silent', '--show-error', '--connect-timeout', '5', '--max-time', '10',
          '--resolve', `${host}:443:${connection.publicIp}`, '--output', '/dev/null', '--write-out', '%{http_code}', `https://${host}/`],
        { signal, timeout: 12_000, maxBuffer: 4096 }, (error, stdout) => resolve(!error && /^[23]\d\d$/.test(stdout.trim())));
      });
    }));
    return { ...status, dns: dns.passed, https: https.every(Boolean), message: 'DNS 与 HTTPS 分别检测；尚未部署时 HTTPS 未通过是预期状态。' };
  }
}
