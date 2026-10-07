import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { arch, cpus, machine, platform, release, totalmem, version } from 'node:os';
import { promisify } from 'node:util';

export interface MachineInfo {
  os: { name: string; version: string; kernel: string };
  hardware: { architecture: string; runtimeArchitecture: string; cpu: string; cores: number; memoryGiB: number };
}

export interface NetworkCheck {
  name: string;
  host: string;
  status: 'reachable' | 'http-error' | 'dns-error' | 'tls-error' | 'timeout' | 'unreachable' | 'cancelled';
  elapsedMs: number;
  httpStatus?: number;
}

export interface ProbeResult {
  machine: MachineInfo;
  checkedAt: string;
  network: { status: 'reachable' | 'limited' | 'unreachable' | 'skipped' | 'cancelled'; checks: NetworkCheck[] };
}

// Fixed, unauthenticated connectivity checks. Never send configuration, tokens,
// kubeconfig, hostnames or hardware information to these endpoints.
const sites = [
  { name: '公共网站', host: 'www.microsoft.com', acceptsNotFound: false },
  { name: '海外访问（Google）', host: 'www.google.com', acceptsNotFound: false },
  { name: 'GitHub API', host: 'api.github.com', acceptsNotFound: false },
  // The root has no asset and may return 404. A TLS + HTTP response here only
  // establishes reachability, not permission to download a private release.
  { name: '安装包下载域名', host: 'release-assets.githubusercontent.com', acceptsNotFound: true },
];

export function architecture(value: string): string {
  return ({ x64: 'amd64', x86_64: 'amd64', aarch64: 'arm64', arm64: 'arm64', ia32: 'x86', i386: 'x86', i686: 'x86' } as Record<string, string>)[value] ?? value;
}

async function systemCommand(file: string, args: string[]): Promise<string> {
  try {
    const result = await promisify(execFile)(file, args, { encoding: 'utf8', timeout: 1500, maxBuffer: 65_536, env: { ...process.env, LC_ALL: 'C' } });
    return result.stdout.trim();
  } catch { return ''; }
}

function cpuModels(values: string[]): string[] {
  return [...new Set(values.map(value => value.trim()).filter(value => value && !/^(unknown|n\/a|unspecified)$/i.test(value)))];
}

export async function linuxCpuModel(models: string[], command = systemCommand): Promise<string> {
  const known = cpuModels(models);
  if (models.length && models.every(model => cpuModels([model]).length)) return known.join(' + ');
  try {
    // ARM /proc/cpuinfo can expose implementer/part IDs without a model name.
    // lscpu decodes those IDs; collect every cluster on heterogeneous CPUs.
    const data = JSON.parse(await command('lscpu', ['--json'])) as { lscpu?: unknown };
    const names: string[] = [];
    const visit = (rows: unknown) => {
      if (!Array.isArray(rows)) return;
      for (const row of rows) {
        if (!row || typeof row !== 'object') continue;
        if (row.field === 'Model name:' && typeof row.data === 'string') names.push(row.data);
        visit(row.children);
      }
    };
    visit(data.lscpu);
    return cpuModels(names).join(' + ') || known.join(' + ');
  } catch { return known.join(' + '); }
}

export async function readMachine(): Promise<MachineInfo> {
  const system = platform();
  let name = ({ darwin: 'macOS', win32: 'Windows', linux: 'Linux' } as Record<string, string>)[system] ?? system;
  let osVersion = '';
  let hardwareArchitecture = architecture(machine());
  if (system === 'darwin') {
    const [product, arm] = await Promise.all([
      systemCommand('/usr/bin/sw_vers', ['-productVersion']),
      systemCommand('/usr/sbin/sysctl', ['-n', 'hw.optional.arm64']),
    ]);
    osVersion = product;
    if (arm === '1') hardwareArchitecture = 'arm64'; // Includes x64 CLI under Rosetta.
  } else if (system === 'linux') {
    try {
      const text = await readFile('/etc/os-release', 'utf8');
      const value = (key: string) => new RegExp(`^${key}=(.*)$`, 'm').exec(text)?.[1]?.trim().replace(/^['"]|['"]$/g, '') ?? '';
      name = value('NAME') || name;
      osVersion = value('VERSION_ID');
    } catch { /* Minimal containers may not have distribution metadata. */ }
  } else if (system === 'win32') { name = version(); }
  const processors = cpus();
  const models = processors.map(processor => processor.model);
  const cpu = system === 'linux' ? await linuxCpuModel(models) : cpuModels(models).join(' + ');
  return {
    os: { name, version: osVersion, kernel: release() },
    hardware: { architecture: hardwareArchitecture, runtimeArchitecture: architecture(arch()),
      cpu, cores: processors.length, memoryGiB: Math.round(totalmem() / 1024 ** 3 * 10) / 10 },
  };
}

type Request = (url: string, options: RequestInit) => Promise<Response>;
export class EnvironmentProbe {
  private active?: { controller: AbortController; result: Promise<ProbeResult> };
  private cached?: ProbeResult;
  constructor(private readonly request: Request = fetch, private readonly host = readMachine, private readonly timeoutMs = 4000) {}

  async run(offline: boolean, refresh = false): Promise<ProbeResult> {
    if (offline) {
      this.cancel();
      return { machine: await this.host(), checkedAt: new Date().toISOString(), network: { status: 'skipped', checks: [] } };
    }
    if (this.active) return this.active.result;
    if (!refresh && this.cached && Date.now() - Date.parse(this.cached.checkedAt) < 30_000) return this.cached;
    const controller = new AbortController();
    const result = this.scan(controller.signal);
    this.active = { controller, result };
    try {
      const snapshot = await result;
      if (!controller.signal.aborted) this.cached = snapshot;
      return snapshot;
    } finally { if (this.active?.controller === controller) this.active = undefined; }
  }

  cancel() {
    this.active?.controller.abort();
    this.active = undefined;
    this.cached = undefined;
  }

  private async scan(signal: AbortSignal): Promise<ProbeResult> {
    const [host, checks] = await Promise.all([this.host(), Promise.all(sites.map(site => this.check(site, signal)))]);
    const count = checks.filter(check => check.status === 'reachable').length;
    return { machine: host, checkedAt: new Date().toISOString(), network: {
      status: signal.aborted ? 'cancelled' : count === sites.length ? 'reachable' : count ? 'limited' : 'unreachable', checks,
    } };
  }

  private async check(site: typeof sites[number], signal: AbortSignal): Promise<NetworkCheck> {
    const started = performance.now();
    const timeout = AbortSignal.timeout(this.timeoutMs);
    const result = (status: NetworkCheck['status'], httpStatus?: number): NetworkCheck => ({ name: site.name, host: site.host, status,
      elapsedMs: Math.round(performance.now() - started), ...(httpStatus === undefined ? {} : { httpStatus }) });
    try {
      const response = await this.request(`https://${site.host}/`, {
        method: 'HEAD', redirect: 'manual', signal: AbortSignal.any([signal, timeout]),
        headers: { 'User-Agent': 'apeiron-cli-connectivity' },
      });
      await response.body?.cancel();
      return result(response.status >= 200 && response.status < 400 || site.acceptsNotFound && response.status === 404 ? 'reachable' : 'http-error', response.status);
    } catch (error) {
      if (signal.aborted) return result('cancelled');
      if (timeout.aborted) return result('timeout');
      const codes = [error, (error as { cause?: unknown } | null)?.cause].map(e => String((e as { code?: unknown } | null)?.code ?? ''));
      if (codes.some(code => /ETIMEDOUT|ESOCKETTIMEDOUT|UND_ERR_(CONNECT|HEADERS|BODY)_TIMEOUT/.test(code))) return result('timeout');
      if (codes.some(code => /ENOTFOUND|EAI_AGAIN|DNS/.test(code))) return result('dns-error');
      if (codes.some(code => /CERT|TLS|SSL/.test(code))) return result('tls-error');
      return result('unreachable'); // Raw proxy/TLS diagnostics may contain credentials.
    }
  }
}
