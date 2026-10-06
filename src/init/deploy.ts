import { spawn, type ChildProcess } from 'node:child_process';
import { writeSync } from 'node:fs';
import { lstat, mkdir, mkdtemp, open, readFile, realpath, unlink, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { APPS, ConfigError, ConfigStore, type Configuration, type DeploymentTarget } from './config';

export interface DeploymentStatus {
  phase: 'idle' | 'preparing' | 'running' | 'succeeded' | 'failed' | 'cancelled';
  startedAt?: string;
  finishedAt?: string;
  exitCode?: number;
  message: string;
  environment?: string;
  log?: string;
  events: string[];
}

function mapping(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ConfigError('环境 values 必须是 YAML 对象。');
  return value as Record<string, unknown>;
}

// Patch only the wizard-owned fields in ONE external file. Defaults, profiles,
// dependencies, templates and release ordering are still resolved by Helmfile.
export function environmentFor(config: Configuration, source: string): string {
  let values: Record<string, unknown>;
  try { values = mapping(Bun.YAML.parse(source)); }
  catch { throw new ConfigError('环境文件必须是有效的普通 YAML；请先将 .gotmpl 模板渲染成 YAML。'); }
  const releases = values.releases === undefined ? {} : mapping(values.releases);
  for (const app of APPS) {
    // Grafana is the portal entry for Chentu's observability component, which
    // contains kps, Loki and Promtail. Helmfile still owns their dependencies.
    for (const name of 'releases' in app ? app.releases : [app.id]) {
      const release = releases[name] === undefined ? {} : mapping(releases[name]);
      releases[name] = { ...release, enabled: config.apps.includes(app.id) };
    }
  }
  return Bun.YAML.stringify({ ...values, tenantSlug: config.slug, releases }, null, 2) + '\n';
}

async function file(path: string, label: string, maxSize?: number) {
  try {
    const info = await lstat(path);
    if (!info.isFile() || (maxSize && info.size > maxSize)) throw new Error();
  } catch { throw new ConfigError(`${label}不存在、不是普通文件或文件过大。请检查路径。`); }
}

export class Deployment {
  private status: DeploymentStatus = { phase: 'idle', message: '', events: [] };
  private child?: ChildProcess;
  private task?: Promise<void>;
  private saving?: Promise<Configuration>;
  private cancelling = false;
  private container?: string;
  constructor(private readonly configPath: string, private readonly onChange?: (status: DeploymentStatus) => void) {}
  get active() { return this.status.phase === 'preparing' || this.status.phase === 'running'; }
  get snapshot(): DeploymentStatus { return { ...this.status, events: [...this.status.events] }; }
  private event(message: string) {
    this.status.message = message;
    this.status.events.push(message);
    if (this.status.events.length > 100) this.status.events.shift();
  }

  async start(save: () => Promise<Configuration>): Promise<void> {
    if (this.active) throw new ConfigError('部署正在进行，请等待当前部署结束。', 409);
    this.cancelling = false;
    this.container = undefined;
    this.status = { phase: 'preparing', startedAt: new Date().toISOString(), message: '正在检查部署配置…', events: [] };
    let config: Configuration;
    try { this.saving = save(); config = await this.saving; }
    catch (error) { this.status = { phase: 'idle', message: '', events: [] }; throw error; }
    finally { this.saving = undefined; }
    this.task = this.execute(config);
  }

  private async execute(config: Configuration) {
    let terminal: DeploymentStatus['phase'] = 'failed';
    let lock: Awaited<ReturnType<typeof open>> | undefined;
    let log: Awaited<ReturnType<typeof open>> | undefined;
    let logFailed = false;
    let lockPath = '';
    try {
      const target = config.deployment!;
      this.event('检查宸途入口、环境文件和部署目标。');
      const script = join(target.root, target.runner === 'docker' ? 'tests/lab/helmfile.sh' : 'deploy/helmfile/run.sh');
      await file(script, '宸途部署脚本');
      await file(target.environment, '环境 values 文件', 2 * 1024 * 1024);
      await new ConfigStore(target.environment).checkLocation();
      if (target.environment.endsWith('.gotmpl')) throw new ConfigError('请提供已渲染的普通 YAML 环境文件，不能直接使用 .gotmpl。');
      if (target.runner === 'native') {
        await file(target.kubeconfig, 'Kubeconfig');
        if (!Bun.which('helmfile', { PATH: process.env.PATH }) || !Bun.which('helm', { PATH: process.env.PATH })) throw new ConfigError('找不到 helmfile 或 helm。请安装到 CLI 的 PATH，或选择本地 Docker 工具箱。');
      } else {
        if (!Bun.which('docker', { PATH: process.env.PATH })) throw new ConfigError('找不到 docker，请检查 CLI 的 PATH。');
        await new ConfigStore(join(target.workDir, 'state.json')).checkLocation();
        await file(join(target.workDir, 'state/kubeconfig'), '工作目录中的 state/kubeconfig（请先准备本地集群）');
      }
      lockPath = await realpath(target.environment) + '.apeiron-deploy.lock';
      try { lock = await open(lockPath, 'wx', 0o600); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'EEXIST') throw new ConfigError('该环境已有部署锁。请等待部署完成；若进程已退出，检查环境文件旁的 .apeiron-deploy.lock。', 409);
        throw error;
      }
      await lock.writeFile(String(process.pid));
      const content = environmentFor(config, await readFile(target.environment, 'utf8'));
      const parent = join(dirname(this.configPath), 'deployments');
      await mkdir(parent, { recursive: true, mode: 0o700 });
      const dir = await mkdtemp(join(parent, 'run-'));
      this.status.environment = join(dir, 'environment.yaml');
      this.status.log = join(dir, 'helmfile.log');
      await writeFile(this.status.environment, content, { mode: 0o600, flag: 'wx' });
      log = await open(this.status.log, 'ax', 0o600);
      this.event('已生成部署环境配置，原环境文件保持不变。');
      if (this.cancelling) throw new Error('cancelled');
      const env = this.environment(target, this.status.environment, dir);
      this.status.phase = 'running';
      this.event('正在运行 Helmfile sync；配置校验、依赖和安装顺序由宸途处理。');
      this.child = spawn('bash', [script, 'sync'], {
        cwd: target.root, env, stdio: ['ignore', 'pipe', 'pipe'], detached: process.platform !== 'win32',
      });
      for (const stream of [this.child.stdout!, this.child.stderr!]) {
        let pending = '';
        stream.on('data', (data: Buffer) => {
          if (log) {
            let offset = 0;
            try { while (offset < data.length) offset += writeSync(log.fd, data, offset, data.length - offset); }
            catch {
              logFailed = true;
              try { if (this.child?.pid) process.kill(process.platform === 'win32' ? this.child.pid : -this.child.pid, 'SIGTERM'); } catch { /* Already exited. */ }
            }
          }
          pending += data.toString('utf8');
          const lines = pending.split('\n');
          pending = (lines.pop() ?? '').slice(-8192);
          for (const line of lines) {
            // Only recognize bounded, non-secret milestones. Helm output and
            // manifest diagnostics remain in the private local log, never HTTP.
            const release = /(?:Upgrading|Installing) release=([a-z0-9-]+),/.exec(line);
            const dependency = /^configuration error: ([a-z0-9-]+) requires enabled release ([a-z0-9/-]+)$/.exec(line.trim());
            if (release) this.event(`正在同步应用：${release[1]}`);
            if (dependency) this.event(`依赖检查未通过：${dependency[1]} 需要启用 ${dependency[2]}。`);
          }
        });
      }
      const code = await new Promise<number>((resolve, reject) => {
        this.child!.once('error', reject);
        this.child!.once('close', code => resolve(code ?? 130));
      });
      this.status.exitCode = code;
      terminal = this.cancelling ? 'cancelled' : code === 0 && !logFailed ? 'succeeded' : 'failed';
      this.event(this.cancelling ? '部署已停止，已完成的变更不会自动回滚。' : logFailed ? '无法写入部署日志，已停止部署。请检查磁盘空间和权限。' : code === 0 ? 'Helmfile 部署完成。' : `Helmfile 部署失败（退出码 ${code}），请查看本机日志并修改配置后重试。`);
    } catch (error) {
      terminal = this.cancelling ? 'cancelled' : 'failed';
      this.event(this.cancelling ? '部署已停止。' : error instanceof ConfigError ? error.message : '无法启动部署，请检查本机路径、权限和部署工具。');
    } finally {
      this.child = undefined;
      await log?.close().catch(() => {});
      if (lock) { await lock.close().catch(() => {}); await unlink(lockPath).catch(() => {}); }
      this.status.phase = terminal;
      this.status.finishedAt = new Date().toISOString();
      this.onChange?.(this.snapshot);
    }
  }

  private environment(target: DeploymentTarget, environment: string, dir: string): NodeJS.ProcessEnv {
    const env = { ...process.env, HELMFILE_NO_COLOR: 'true', HELMFILE_LOG_LEVEL: 'info' };
    if (target.runner === 'native') return { ...env, CHENTU_ROOT: target.root, CHENTU_ENV: environment, CHENTU_PROFILE: target.profile, KUBECONFIG: target.kubeconfig };
    this.container = 'apeiron-init-' + dir.split('/').pop()!.toLowerCase();
    return { ...env, LAB_ENV: environment, LAB_WORK_DIR: target.workDir, LAB_IMAGE: target.image, LAB_CONTAINER: this.container };
  }

  async stop() {
    if (!this.active) { await this.task; return; }
    this.cancelling = true;
    await this.saving?.catch(() => {});
    const pid = this.child?.pid;
    const kill = (signal: NodeJS.Signals) => {
      try { if (pid) process.kill(process.platform === 'win32' ? pid : -pid, signal); } catch { /* Already exited. */ }
    };
    kill('SIGTERM');
    const timer = setTimeout(() => kill('SIGKILL'), 5000);
    try {
      if (this.container && pid) {
        // Only remove the toolbox container created for this deployment.
        const cleanup = spawn('docker', ['rm', '-f', this.container], { stdio: 'ignore' });
        cleanup.on('error', () => {});
      }
      await this.task;
    } finally { clearTimeout(timer); }
  }
}
