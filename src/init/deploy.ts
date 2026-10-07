import { publicAccessPhase } from './public-access';
import { spawn, type ChildProcess } from 'node:child_process';
import { writeSync } from 'node:fs';
import { appendFile, lstat, mkdir, mkdtemp, open, readFile, realpath, unlink, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { resolveChentu, type ResourceResolver } from '../resources/chentu';
import { bootstrapFresh, finishClusterAccess, prepareFreshInstallation, type RunCommand } from './bootstrap';
import { APPS, ConfigError, ConfigStore, type Configuration, type DeploymentTarget } from './config';
import { collectAccess, type AccessArtifacts, type AccessInfo } from './access';
import { readInitialAdmin } from './verification';
import { checkHelmState, removeToolbox } from './helm-state';

export interface DeploymentStatus {
  phase: 'idle' | 'preparing' | 'running' | 'stopping' | 'succeeded' | 'failed' | 'cancelled';
  stopFailed?: boolean;
  startedAt?: string;
  finishedAt?: string;
  exitCode?: number;
  message: string;
  environment?: string;
  log?: string;
  events: string[];
  access?: AccessInfo;
}

function mapping(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ConfigError('环境 values 必须是 YAML 对象。');
  return value as Record<string, unknown>;
}

// Patch only the wizard-owned fields in ONE external file. Defaults, topology presets,
// dependencies, templates and release ordering are still resolved by Helmfile.
export function environmentFor(config: Configuration, source: string): string {
  let values: Record<string, unknown>;
  try { values = mapping(Bun.YAML.parse(source)); }
  catch { throw new ConfigError('环境文件必须是有效的普通 YAML；请先将 .gotmpl 模板渲染成 YAML。'); }
  if ('profile' in values) throw new ConfigError('环境文件中的 profile 已移除，请使用 topology。');
  const releases = values.releases === undefined ? {} : mapping(values.releases);
  for (const app of APPS) {
    // Grafana is the portal entry for Chentu's observability component, which
    // contains kps, Loki and Promtail. Helmfile still owns their dependencies.
    for (const name of 'releases' in app ? app.releases : [app.id]) {
      const release = releases[name] === undefined ? {} : mapping(releases[name]);
      releases[name] = { ...release, enabled: config.apps.includes(app.id) };
    }
  }
  if (config.deployment?.installation && config.apps.includes('vasi')) {
    const access = releases['cluster-access'] === undefined ? {} : mapping(releases['cluster-access']);
    releases['cluster-access'] = { ...access, enabled: true };
  }
  if (config.deployment) {
    const nexus = mapping(releases.nexus);
    const nexusValues = nexus.values === undefined ? {} : mapping(nexus.values);
    releases.nexus = { ...nexus, values: { ...nexusValues, publicProxies: !config.deployment.offline } };
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
  private stopTask?: Promise<void>;
  private releaseLock?: () => Promise<void>;
  private saving?: Promise<Configuration>;
  private cancelling = false;
  private container?: string;
  private preparation = new AbortController();
  private access?: AccessArtifacts;
  private completedTarget?: DeploymentTarget;
  private recordEvent?: (message: string) => void;
  constructor(private readonly configPath: string, private readonly onChange?: (status: DeploymentStatus) => void,
    private readonly resources: ResourceResolver = resolveChentu, private readonly adminReader = readInitialAdmin) {}
  get active() { return ['preparing', 'running', 'stopping'].includes(this.status.phase) || Boolean(this.stopTask); }
  get snapshot(): DeploymentStatus { return { ...this.status, events: [...this.status.events] }; }
  download(kind: 'ca' | 'hosts'): string | undefined { return this.status.phase === 'succeeded' ? this.access?.[kind] : undefined; }
  async initialAdmin(signal: AbortSignal) {
    if (this.status.phase !== 'succeeded' || !this.completedTarget) throw new ConfigError('部署成功后才可读取初始管理员凭据。', 409);
    return this.adminReader(this.completedTarget, signal);
  }
  private event(message: string) {
    this.status.message = message;
    this.status.events.push(message);
    if (this.status.events.length > 100) this.status.events.shift();
    this.recordEvent?.(message);
  }

  async start(save: () => Promise<Configuration>, preflightMessage?: string): Promise<void> {
    if (this.active) throw new ConfigError('部署正在进行，请等待当前部署结束。', 409);
    const previous = this.status;
    this.cancelling = false;
    this.preparation = new AbortController();
    this.container = undefined;
    this.access = undefined;
    this.completedTarget = undefined;
    this.task = undefined;
    this.status = { phase: 'preparing', startedAt: new Date().toISOString(), message: '正在检查部署配置…', events: [] };
    let config: Configuration;
    try { this.saving = save(); config = await this.saving; }
    catch (error) { if (!this.cancelling) this.status = previous; throw error; }
    finally { this.saving = undefined; }
    this.task = this.execute(config, preflightMessage);
  }

  private async execute(config: Configuration, preflightMessage?: string) {
    let terminal: DeploymentStatus['phase'] = 'failed';
    let lock: Awaited<ReturnType<typeof open>> | undefined;
    let log: Awaited<ReturnType<typeof open>> | undefined;
    let logFailed = false;
    let lockPath = '';
    try {
      let target = { ...config.deployment! };
      const parent = join(dirname(this.configPath), 'deployments');
      await mkdir(parent, { recursive: true, mode: 0o700 });
      const dir = await mkdtemp(join(parent, 'run-'));
      this.status.log = join(dir, 'install.log');
      log = await open(this.status.log, 'ax', 0o600);
      this.recordEvent = message => {
        const line = Buffer.from(`[${new Date().toISOString()}] ${message}\n`);
        try {
          let offset = 0;
          while (offset < line.length) offset += writeSync(log!.fd, line, offset, line.length - offset);
        } catch {
          logFailed = true;
          try { if (this.child?.pid) process.kill(process.platform === 'win32' ? this.child.pid : -this.child.pid, 'SIGTERM'); } catch { /* Exited. */ }
        }
      };
      this.event('开始安装检查。');
      if (preflightMessage) this.event(preflightMessage);
      const run: RunCommand = async (command, args, cwd, env) => {
        this.preparation.signal.throwIfAborted();
        if (env.LAB_CONTAINER) this.container = env.LAB_CONTAINER;
        if (command === 'docker' && args[0] === 'run' && args.includes('--name')) this.container = args[args.indexOf('--name') + 1];
        this.child = spawn(command, args, { cwd, env, stdio: ['ignore', 'pipe', 'pipe'], detached: process.platform !== 'win32' });
        for (const stream of [this.child.stdout!, this.child.stderr!]) stream.on('data', (data: Buffer) => {
          try {
            let offset = 0;
            while (offset < data.length) offset += writeSync(log!.fd, data, offset, data.length - offset);
          } catch {
            logFailed = true;
            try { if (this.child?.pid) process.kill(process.platform === 'win32' ? this.child.pid : -this.child.pid, 'SIGTERM'); } catch { /* Exited. */ }
          }
        });
        const child = this.child;
        const code = await new Promise<number>((resolve, reject) => { child.once('error', reject); child.once('close', code => resolve(code ?? 130)); });
        if (logFailed) throw new ConfigError('无法写入安装日志，已停止安装。');
        this.preparation.signal.throwIfAborted();
        this.child = undefined;
        return code;
      };
      let freshEnv: NodeJS.ProcessEnv | undefined;
      if (target.installation) {
        lockPath = this.configPath + '.installation.lock';
        try { lock = await open(lockPath, 'wx', 0o600); }
        catch { throw new ConfigError('此配置已有安装进程，请等待完成或检查安装锁。', 409); }
        await lock.writeFile(String(process.pid));
        const context = { directory: dir, installationKey: this.configPath, signal: this.preparation.signal, progress: (message: string) => this.event(message), run, resources: this.resources };
        const prepared = await prepareFreshInstallation(config, context);
        target = prepared.target;
        await writeFile(target.environment, environmentFor(config, prepared.source), { mode: 0o600, flag: 'wx' });
        this.status.environment = target.environment;
        freshEnv = await bootstrapFresh(target, target.environment, context, prepared.runtime);
      }
      this.event('检查宸途入口、环境文件和部署目标。');
      await file(target.environment, '环境 values 文件', 2 * 1024 * 1024);
      await new ConfigStore(target.environment).checkLocation();
      if (target.environment.endsWith('.gotmpl')) throw new ConfigError('请提供已渲染的普通 YAML 环境文件，不能直接使用 .gotmpl。');
      if (target.runner === 'native') {
        await file(target.kubeconfig, 'Kubeconfig');
        if (!Bun.which('helmfile', { PATH: freshEnv?.PATH || process.env.PATH }) || !Bun.which('helm', { PATH: freshEnv?.PATH || process.env.PATH })) throw new ConfigError('找不到 helmfile 或 helm。请安装到 CLI 的 PATH，或选择本地 Docker 工具箱。');
      } else {
        if (!Bun.which('docker', { PATH: process.env.PATH })) throw new ConfigError('找不到 docker，请检查 CLI 的 PATH。');
        await new ConfigStore(join(target.workDir, 'state.json')).checkLocation();
        await file(join(target.workDir, 'state/kubeconfig'), '工作目录中的 state/kubeconfig（请先准备本地集群）');
      }
      if (!lock) {
        lockPath = await realpath(target.environment) + '.apeiron-deploy.lock';
        try { lock = await open(lockPath, 'wx', 0o600); }
        catch (error) {
          if ((error as NodeJS.ErrnoException).code === 'EEXIST') throw new ConfigError('该环境已有部署锁。请等待部署完成；若进程已退出，检查环境文件旁的 .apeiron-deploy.lock。', 409);
          throw error;
        }
        await lock.writeFile(String(process.pid));
      }
      const content = environmentFor(config, await readFile(target.environment, 'utf8'));
      if (!target.installation) target.root = await this.resources(target.root, this.preparation.signal, message => this.event(message), target);
      this.preparation.signal.throwIfAborted();
      const script = join(target.root, target.runner === 'docker' ? 'tests/lab/helmfile.sh' : 'deploy/helmfile/run.sh');
      await file(script, '宸途部署脚本');
      this.status.environment = join(dir, 'environment.yaml');
      await writeFile(this.status.environment, content, { mode: 0o600, flag: 'wx' });
      this.event('已生成部署环境配置，原环境文件保持不变。');
      if (this.cancelling) throw new Error('cancelled');
      const env = freshEnv ? { ...freshEnv, CHENTU_ENV: this.status.environment, ...(target.runner === 'docker' ? { LAB_ENV: this.status.environment } : {}) } : this.environment(target, this.status.environment, dir);
      if (freshEnv?.LAB_CONTAINER) this.container = freshEnv.LAB_CONTAINER;
      this.event('检查 Helm 是否存在未完成的安装、升级或回滚。');
      await checkHelmState(target, env, this.preparation.signal);
      this.preparation.signal.throwIfAborted();
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
            // manifest diagnostics stay in the log, available through the
            // explicit token-protected log viewer, not the status response.
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
      this.event(this.cancelling ? '部署命令已退出，正在确认工具箱清理。' : logFailed ? '无法写入部署日志，已停止部署。请检查磁盘空间和权限。' : code === 0 ? 'Helmfile 部署完成。' : `Helmfile 部署失败（退出码 ${code}），请查看本机日志并修改配置后重试。`);
      if (terminal === 'succeeded' && target.installation) {
        if (config.apps.includes('vasi')) {
          // Keep installation active (and stoppable) until real OIDC authentication and RBAC pass.
          terminal = 'failed';
          this.status.exitCode = undefined;
          this.event('正在配置集群 SSO，并验证 Vasi 登录与管理员、只读用户的权限。');
          await finishClusterAccess(target, dir, env, run);
          this.status.exitCode = 0;
          terminal = 'succeeded';
        }
        if (target.installation.publicAccess) {
          terminal = 'failed';
          this.status.exitCode = undefined;
          this.event('配置 Caddy 公网入口并验证 Apeiron / IAM 的 HTTPS。');
          await publicAccessPhase('finish', target, dir, env, run, this.preparation.signal);
          terminal = 'succeeded';
          this.status.exitCode = 0;
        }
        this.event('正在准备应用入口、CA 证书和本机解析指引。');
        this.access = await collectAccess(target, dir, this.preparation.signal);
        this.status.access = this.access.info;
        this.completedTarget = target;
        if (this.cancelling) terminal = 'cancelled';
        this.event(this.cancelling ? '正在停止部署并清理工具箱。' : target.installation.publicAccess ? '公网 HTTPS 已验证，可以进入应用测试。' : 'Helmfile 部署完成，请按访问指引配置 DNS 和证书信任。');
      }
    } catch (error) {
      terminal = this.cancelling ? 'cancelled' : 'failed';
      this.event(this.cancelling ? '部署操作已中断，正在确认清理。' : error instanceof ConfigError ? error.message : '无法启动部署，请检查本机路径、权限和部署工具。');
    } finally {
      this.child = undefined;
      let ownsLock = Boolean(lock);
      this.releaseLock = async () => {
        if (lock) { await lock.close(); lock = undefined; }
        if (ownsLock) { await unlink(lockPath).catch(error => { if (error.code !== 'ENOENT') throw error; }); ownsLock = false; }
        this.releaseLock = undefined;
      };
      // Cancellation owns cleanup and the lock until both the process and its
      // Docker toolbox have exited. A failed cleanup keeps restart blocked.
      if (!this.cancelling) {
        if (this.container && !await removeToolbox(this.container)) {
          this.cancelling = true;
          this.status.stopFailed = true;
          this.event('无法确认部署工具箱已停止，请检查 Docker 后点击“重试停止”。暂时不能重新部署。');
        } else if (!this.cancelling) {
          try { await this.releaseLock(); }
          catch { this.cancelling = true; this.status.stopFailed = true; this.event('无法释放部署锁，请检查目录权限后点击“重试停止”。'); }
        }
      }
      this.recordEvent = undefined;
      await log?.close().catch(() => {});
      if (this.cancelling) { this.status.phase = 'stopping'; return; }
      this.status.phase = terminal;
      this.status.finishedAt = new Date().toISOString();
      this.onChange?.(this.snapshot);
    }
  }

  private environment(target: DeploymentTarget, environment: string, dir: string): NodeJS.ProcessEnv {
    const env = { ...process.env, HELMFILE_NO_COLOR: 'true', HELMFILE_LOG_LEVEL: 'info' };
    if (target.runner === 'native') return { ...env, CHENTU_ROOT: target.root, CHENTU_ENV: environment, KUBECONFIG: target.kubeconfig };
    this.container = 'apeiron-init-' + dir.split('/').pop()!.toLowerCase();
    return { ...env, LAB_ENV: environment, LAB_WORK_DIR: target.workDir, LAB_IMAGE: target.image, LAB_CONTAINER: this.container };
  }

  stop(): Promise<void> {
    if (this.stopTask) return this.stopTask;
    if (!this.active) return this.task ?? Promise.resolve();
    this.cancelling = true;
    this.status.phase = 'stopping';
    this.status.stopFailed = false;
    this.event('正在停止部署进程；已提交给集群的任务可能继续运行。');
    this.preparation.abort();
    this.stopTask = this.stopCurrent().finally(() => { this.stopTask = undefined; });
    return this.stopTask;
  }

  private async stopCurrent() {
    await this.saving?.catch(() => {});
    const pid = this.child?.pid;
    const kill = (signal: NodeJS.Signals) => {
      try { if (pid) process.kill(process.platform === 'win32' ? pid : -pid, signal); } catch { /* Already exited. */ }
    };
    kill('SIGTERM');
    const timer = setTimeout(() => kill('SIGKILL'), 5000);
    try {
      const cleanup = this.container ? removeToolbox(this.container) : Promise.resolve(true);
      await this.task;
      kill('SIGKILL'); // Include any descendants that outlived the direct child.
      if (!await cleanup) throw new Error('cleanup');
      await this.releaseLock?.();
      this.status.phase = 'cancelled';
      this.status.stopFailed = false;
      this.status.finishedAt = new Date().toISOString();
      this.access = undefined; this.completedTarget = undefined; delete this.status.access;
      this.event('部署已停止。已完成的变更保留，集群任务可能继续运行；重新部署会先检查 Helm 状态。');
    } catch {
      this.status.phase = 'stopping';
      this.status.stopFailed = true;
      this.event('无法确认部署工具箱已停止或释放部署锁，请检查 Docker 和目录权限后点击“重试停止”。暂时不能重新部署。');
    } finally {
      clearTimeout(timer);
      if (this.status.log) await appendFile(this.status.log, `[${new Date().toISOString()}] ${this.status.message}\n`).catch(() => {});
      this.onChange?.(this.snapshot);
    }
  }
}
