import { execFile } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { constants } from 'node:fs';
import { open, stat } from 'node:fs/promises';
import { hostname } from 'node:os';
import { promisify } from 'node:util';
import { type AccessArtifacts, publicCa } from './access';
import { ConfigError } from './config';
import { checkDns } from './dns';
import { safeDomain, safeEntryIp } from './installation';

export interface LocalAccessCapability { available: boolean; host: string; reason: string }
export interface LocalAccessStatus {
  phase: 'idle' | 'installing' | 'succeeded' | 'failed' | 'cancelled';
  message: string;
  backup?: string;
  checks?: { host: string; passed: boolean }[];
}
export async function localAccessCapability(): Promise<LocalAccessCapability> {
  let available = false;
  if (process.platform === 'darwin' && !process.env.SSH_CONNECTION && !process.env.SSH_TTY) {
    try { available = (await stat('/dev/console')).uid === process.getuid?.(); } catch { /* No desktop session. */ }
  }
  return { available, host: hostname(), reason: available ? '' : '一键安装需要在有桌面会话的 Mac 上运行 CLI。请在访问平台的电脑上按手动指引配置。' };
}

export function accessHostnames(artifacts: AccessArtifacts): string[] {
  const { domain, entryIp, local } = artifacts.info;
  if (!safeDomain(domain) || !safeEntryIp(entryIp) || (local && entryIp !== '127.0.0.1')) throw new ConfigError('部署访问地址无效。');
  if (!artifacts.ca || !artifacts.hosts || artifacts.hosts.length > 65_536) throw new ConfigError('本次部署未生成完整的 CA 和 hosts 文件，请检查部署日志。');
  const ca = publicCa(artifacts.ca);
  if (ca.fingerprint !== artifacts.info.ca?.fingerprint) throw new ConfigError('部署 CA 指纹不匹配。');
  const names = artifacts.hosts.split(/\r?\n/).filter(line => line.trim() && !line.trim().startsWith('#')).map(line => {
    const parts = line.trim().split(/\s+/);
    const name = parts[1];
    if (parts.length !== 2 || parts[0] !== entryIp || !safeDomain(name) || !name!.endsWith('.' + domain)) throw new ConfigError('部署 hosts 清单无效。');
    return name!;
  });
  if (!names.includes(`apeiron.${domain}`) || !names.includes(`iam.${domain}`)) throw new ConfigError('部署 hosts 清单缺少 Apeiron 或 IAM。');
  return [...new Set(names)].sort();
}

// Replace only this domain's managed block. Existing aliases/comments and other
// deployments survive; conflicting manual records require an explicit edit.
export function mergeHosts(current: string, artifacts: AccessArtifacts): string {
  if (current.includes('\0') || Buffer.byteLength(current) > 1_048_576) throw new ConfigError('本机 hosts 文件格式异常。');
  const names = accessHostnames(artifacts);
  const { domain, entryIp } = artifacts.info;
  const begin = `# BEGIN APEIRON ${domain}`, end = `# END APEIRON ${domain}`;
  const lines = current.split('\n');
  const keep: string[] = [];
  let inside = false, seen = false;
  for (const line of lines) {
    const clean = line.replace(/\r$/, '');
    if (clean === begin) {
      if (seen || inside) throw new ConfigError('hosts 中的 Apeiron 配置标记重复，请先检查文件。');
      seen = inside = true; continue;
    }
    if (clean === end) {
      if (!inside) throw new ConfigError('hosts 中的 Apeiron 配置标记不完整，请先检查文件。');
      inside = false; continue;
    }
    if (inside) {
      const record = clean.split('#')[0]!.trim().split(/\s+/);
      if (record.length > 1 && record.slice(1).some(name => !name.toLowerCase().endsWith('.' + domain))) throw new ConfigError('Apeiron hosts 区块中包含其他域名，请先检查文件。');
      continue;
    }
    const [address, ...aliases] = clean.split('#')[0]!.trim().split(/\s+/);
    if (address !== entryIp && aliases.some(name => names.includes(name.toLowerCase()))) throw new ConfigError('hosts 已有指向其他 IP 的平台域名。请先在手动配置中检查冲突记录。');
    keep.push(line);
  }
  if (inside) throw new ConfigError('hosts 中的 Apeiron 配置标记不完整，请先检查文件。');
  while (keep.at(-1) === '') keep.pop();
  return keep.join('\n') + '\n\n' + [begin, ...names.map(name => `${entryIp} ${name}`), end, ''].join('\n');
}

const hash = (text: string) => createHash('sha256').update(text).digest('hex');
const shellQuote = (value: string) => "'" + value.replaceAll("'", "'\\''") + "'";
const appleQuote = (value: string) => '"' + value.replaceAll('\\', '\\\\').replaceAll('"', '\\"').replaceAll('\n', '\\n').replaceAll('\r', '\\r') + '"';

export function macAccessScript(current: string, artifacts: AccessArtifacts): string {
  const merged = mergeHosts(current, artifacts);
  const ca = publicCa(artifacts.ca!).pem;
  const delimiter = `APEIRON_${randomBytes(24).toString('hex')}`;
  // Embed validated data in the privileged command itself: root never executes
  // a script or reads a certificate from a user-writable temporary directory.
  return `set -eu
export PATH=/usr/bin:/bin:/usr/sbin:/sbin
umask 077
hosts=/private/etc/hosts
[ -f "$hosts" ] && [ ! -L "$hosts" ] || { echo APEIRON_HOSTS_CHANGED >&2; exit 1; }
expected=${shellQuote(hash(current))}
actual=$(/usr/bin/shasum -a 256 "$hosts"); actual=\${actual%% *}
[ "$actual" = "$expected" ] || { echo APEIRON_HOSTS_CHANGED >&2; exit 1; }
work=$(/usr/bin/mktemp -d /private/etc/apeiron-access.XXXXXX)
trap '/bin/rm -f "$work/ca.crt" "$work/hosts.new"' EXIT
/bin/cp -p "$hosts" "$work/hosts.before"
/bin/cp -p "$hosts" "$work/hosts.new"
/bin/cat > "$work/hosts.new" <<'${delimiter}'
${merged}${delimiter}
/bin/cat > "$work/ca.crt" <<'${delimiter}'
${ca.trim()}\n${delimiter}
/usr/bin/security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain "$work/ca.crt" || { echo APEIRON_CA_FAILED >&2; exit 1; }
actual=$(/usr/bin/shasum -a 256 "$hosts"); actual=\${actual%% *}
[ "$actual" = "$expected" ] && [ ! -L "$hosts" ] || { echo APEIRON_HOSTS_CHANGED >&2; exit 1; }
/bin/mv -f "$work/hosts.new" "$hosts"
/usr/bin/dscacheutil -flushcache
/usr/bin/killall -HUP mDNSResponder || true
echo "APEIRON_BACKUP=$work/hosts.before"
`;
}

async function readHosts(): Promise<string> {
  const file = await open('/private/etc/hosts', constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const info = await file.stat();
    if (!info.isFile() || info.size > 1_048_576) throw new ConfigError('本机 hosts 文件格式异常。');
    return await file.readFile('utf8');
  } finally { await file.close(); }
}
async function authorize(script: string): Promise<string> {
  const { stdout } = await promisify(execFile)('/usr/bin/osascript', ['-e',
    macAuthorizationScript(script)],
  { timeout: 180_000, maxBuffer: 8192 });
  return stdout;
}
export function macAuthorizationScript(script: string) {
  return `do shell script ${appleQuote(script)} with administrator privileges with prompt "Apeiron 将配置本机 hosts，并在系统钥匙串中信任部署 CA（HTTPS）。"`;
}
async function verify(artifacts: AccessArtifacts) {
  const dns = await checkDns(artifacts.info);
  // A workstation hosts installation does not configure the network's wildcard DNS.
  return Promise.all(dns.checks.slice(0, 2).map(async check => {
    if (check.status !== 'matched') return { host: check.host, passed: false };
    try {
      const { stdout } = await promisify(execFile)('/usr/bin/curl', ['--disable', '--silent', '--show-error', '--head', '--noproxy', '*', '--max-time', '10', '--output', '/dev/null', '--write-out', '%{http_code}', `https://${check.host}${(artifacts.info.httpsPort ?? 443) === 443 ? '' : ':' + artifacts.info.httpsPort}/`],
        { timeout: 12_000, maxBuffer: 4096, env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin' } });
      return { host: check.host, passed: /^[23]\d\d$/.test(stdout.trim()) };
    } catch { return { host: check.host, passed: false }; }
  }));
}

export class LocalAccessInstaller {
  private status: LocalAccessStatus = { phase: 'idle', message: '' };
  private task?: Promise<void>;
  constructor(private readonly operations = { capability: localAccessCapability, readHosts, authorize, verify }) {}
  capability() { return this.operations.capability(); }
  get active() { return this.status.phase === 'installing'; }
  get snapshot(): LocalAccessStatus { return { ...this.status }; }
  reset() { if (!this.active) this.status = { phase: 'idle', message: '' }; }
  async start(artifacts: AccessArtifacts) {
    if (this.active) throw new ConfigError('本机访问配置正在进行，请等待系统授权完成。', 409);
    accessHostnames(artifacts);
    this.status = { phase: 'installing', message: '请在 CLI 主机的 macOS 系统弹窗中授权。授权后会配置 hosts、信任 CA 并检查 HTTPS。' };
    try {
      const capability = await this.operations.capability();
      if (!capability.available) throw new ConfigError(capability.reason);
    } catch (error) { this.resetAfterError(); throw error; }
    this.task = this.install(artifacts);
  }
  private resetAfterError() { this.status = { phase: 'idle', message: '' }; }
  private async install(artifacts: AccessArtifacts) {
    try {
      const script = macAccessScript(await this.operations.readHosts(), artifacts);
      const output = await this.operations.authorize(script);
      const backup = /APEIRON_BACKUP=(\/private\/etc\/apeiron-access\.[A-Za-z0-9]+\/hosts\.before)/.exec(output)?.[1];
      this.status = { ...this.status, backup, message: '配置已写入，正在检查本机解析与 HTTPS…' };
      const checks = await this.operations.verify(artifacts);
      this.status = { ...this.status, checks, phase: checks.every(check => check.passed) ? 'succeeded' : 'failed', message: checks.every(check => check.passed)
        ? '本机解析与 HTTPS 检查通过，可以打开 Apeiron。' : '配置已安装，但访问检查未全部通过。请确认应用状态，或展开手动配置进行检查；可以重试。' };
    } catch (error) {
      const detail = String(error instanceof Error ? error.message : '') + String((error as { stderr?: string })?.stderr ?? '');
      const cancelled = /\(-128\)/.test(detail);
      this.status = { phase: cancelled ? 'cancelled' : 'failed', message: cancelled ? '已取消系统授权，未安装本机访问配置。可以再次点击安装。'
        : error instanceof ConfigError ? error.message : detail.includes('APEIRON_HOSTS_CHANGED') ? 'hosts 文件在配置期间发生变化，未覆盖。CA 可能已安装，请检查后重试。'
        : detail.includes('APEIRON_CA_FAILED') ? 'CA 信任安装失败，hosts 未修改。请检查系统权限后重试。'
        : '本机访问配置未完成，请检查系统授权或使用手动配置。部分配置可能已安装，重试会合并现有记录。' };
    }
  }
  async wait() { await this.task; }
}
