import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { ConfigError } from './config';
import { safeHost, safeName, validateConnection } from './installation';

export interface NodeFacts {
  host: string; name: string; os: string; version: string; architecture: string;
  cores: number; memoryGiB: number; diskGiB: number; addresses: string[];
  sudo: boolean; existingCluster: boolean; supported: boolean; error?: string;
}
export type Connection = ReturnType<typeof validateConnection>;

export function nodeArchitecture(architecture: string): 'amd64' | 'arm64' {
  if (['x64', 'x86_64', 'amd64'].includes(architecture)) return 'amd64';
  if (['arm64', 'aarch64'].includes(architecture)) return 'arm64';
  throw new ConfigError(`不支持的 CPU 架构：${architecture}`);
}

// Read-only probe. No dependency installation, cluster joins or firewall edits.
const probeScript = `import os,json,platform,socket,subprocess,shutil
def run(args):
 try: return subprocess.run(args,capture_output=True,text=True,timeout=5)
 except Exception: return None
release={}
try:
 for line in open('/etc/os-release'):
  if '=' in line:
   k,v=line.strip().split('=',1); release[k]=v.strip('"')
except OSError: pass
addresses=[]
r=run(['ip','-j','-4','address','show','scope','global'])
if r and r.returncode==0:
 for iface in json.loads(r.stdout):
  if iface.get('ifname','').startswith(('docker','br-','cni','flannel','veth')): continue
  addresses += [a['local'] for a in iface.get('addr_info',[]) if a.get('family')=='inet']
memory=0
try: memory=os.sysconf('SC_PAGE_SIZE')*os.sysconf('SC_PHYS_PAGES')/1024**3
except (ValueError,OSError): pass
r=run(['sudo','-n','true']) if os.geteuid()!=0 else None
print(json.dumps(dict(name=socket.gethostname().split('.')[0].lower(),os=release.get('ID',platform.system().lower()),version=release.get('VERSION_ID',platform.release()),architecture=platform.machine(),cores=os.cpu_count() or 0,memoryGiB=round(memory,1),diskGiB=round(shutil.disk_usage('/').free/1024**3,1),addresses=addresses,sudo=os.geteuid()==0 or bool(r and r.returncode==0),existingCluster=os.path.exists('/etc/rancher/k3s') or os.path.exists('/var/lib/rancher/k3s'))))
`;

function execute(command: string, args: string[], input: string, signal: AbortSignal): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = execFile(command, args, { timeout: 20_000, maxBuffer: 128 * 1024, signal, encoding: 'utf8' }, (error, stdout) => error ? reject(error) : resolve(stdout));
    child.stdin?.on('error', () => {});
    child.stdin?.end(input);
  });
}

export function sshArgs(host: string, connection: Connection) {
  if (!safeHost(host)) throw new ConfigError('节点地址格式不正确。');
  return ['-T', '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes', '-o', 'ConnectTimeout=8',
    '-p', String(connection.sshPort),
    ...(connection.sshUser ? ['-l', connection.sshUser] : []),
    ...(connection.sshKey ? ['-i', connection.sshKey, '-o', 'IdentitiesOnly=yes'] : []), host, 'python3 -'];
}

export async function probeNode(host: string, connection: Connection, signal: AbortSignal, local = false): Promise<NodeFacts> {
  try {
    const raw = await execute(local ? 'python3' : 'ssh', local ? ['-'] : sshArgs(host, connection), probeScript, signal);
    const facts = JSON.parse(raw) as NodeFacts;
    if (!safeName(facts.name) || !Array.isArray(facts.addresses) || !['cores','memoryGiB','diskGiB'].every(key => Number.isFinite(facts[key as 'cores']))) throw new Error();
    const supported = facts.os === 'ubuntu' && facts.version === '22.04' && facts.architecture === 'x86_64';
    return { ...facts, host, supported, error: !supported ? '安装器当前支持 Ubuntu 22.04 / AMD64。' : !facts.sudo ? '需要 root 或免密 sudo。' : facts.existingCluster ? '检测到已有 K3s 数据，请使用未安装集群的机器。' : !facts.addresses.length ? '未发现可用的内网 IPv4。' : undefined };
  } catch {
    signal.throwIfAborted();
    return { host, name: '', os: '', version: '', architecture: '', cores: 0, memoryGiB: 0, diskGiB: 0, addresses: [], sudo: false, existingCluster: false, supported: false,
      error: local ? '无法检测本机，请检查 Python 3。' : 'SSH 检测失败：检查地址、用户、密钥和 Python 3；首次连接请先用 ssh 确认主机指纹。' };
  }
}

export async function probeNodes(input: unknown, signal: AbortSignal) {
  if (!input || typeof input !== 'object') throw new ConfigError('节点检测参数不正确。');
  const value = input as Record<string, unknown>;
  const connection = validateConnection(value);
  if (!Array.isArray(value.hosts) || !value.hosts.length || value.hosts.length > 32 || !value.hosts.every(safeHost) || new Set(value.hosts).size !== value.hosts.length) throw new ConfigError('请填写 1–32 个不重复的 IP 或 SSH 别名，每行一个。');
  const results: NodeFacts[] = [];
  for (let i = 0; i < value.hosts.length; i += 4) results.push(...await Promise.all(value.hosts.slice(i, i + 4).map(host => probeNode(host, connection, signal))));
  return results;
}

export async function sshAliases(): Promise<string[]> {
  // No config evaluation: ignore wildcards, Match, Include and ProxyCommand.
  // Explicit aliases still use OpenSSH's normal configuration during a probe.
  let source: string;
  try { source = await readFile(join(homedir(), '.ssh/config'), 'utf8'); } catch { return []; }
  if (source.length > 512 * 1024) throw new ConfigError('SSH 配置过大，请手动填写地址。');
  return [...new Set(source.split('\n').flatMap(line => /^\s*Host\s+/i.test(line) ? line.trim().split(/\s+/).slice(1).filter(safeHost) : []))].slice(0, 32);
}
