import { createHash, randomBytes } from 'node:crypto';
import { lstat, mkdir, open, readFile, realpath, rename, unlink } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { installationDefaults, validateInstallation, type Installation } from './installation';

// IDs match Chentu releases (Grafana is provided by kps). Deployment dependencies
// remain owned by Helmfile; this is only the user's requested application set.
export const APPS = [
  { id: 'nexus', name: 'Nexus', description: '制品与镜像仓库', selected: true, required: true },
  { id: 'vasi', name: 'Vasi', description: '集群资源与 Kubernetes 管理', selected: true, required: true },
  { id: 'ontology', name: 'Limani', description: '本体与业务建模', selected: true, required: true },
  { id: 'apeiron', name: 'Apeiron', description: 'AI 工作台与平台管理（含 Ops）', selected: true, required: true },
  { id: 'task', name: 'Task', description: '项目与任务协作', selected: true, required: false },
  { id: 'corpus', name: 'Corpus', description: '文档、知识检索与团队资料', selected: true, required: false },
  { id: 'matrix', name: 'Chat', description: '团队消息与房间协作', selected: true, required: false },
  { id: 'files', name: 'Files', description: '私人文件与团队共享文件', selected: true, required: false },
  { id: 'stalwart', name: '邮件', description: '收件箱与邮件收发', selected: true, required: false },
  { id: 'gateway', name: 'Gateway', description: '模型、访问凭证与用量管理', selected: false, required: false },
  { id: 'filer', name: 'Filer', description: '对象存储管理', selected: false, required: false },
  { id: 'git', name: '代码仓库', description: 'Git 代码托管与代码评审', selected: false, required: false },
  { id: 'gpustack', name: 'GPUStack', description: 'GPU 模型部署与推理服务管理', selected: false, required: false },
  { id: 'langfuse', name: 'Langfuse', description: '模型调用追踪、评测与提示词管理', selected: false, required: false },
  { id: 'kps', name: 'Grafana', description: '监控仪表盘、指标与日志', selected: false, required: false, releases: ['kps', 'loki', 'promtail'] },
] as const;

export interface Configuration {
  schemaVersion: 2;
  slug: string;
  // Retain legacy credentials on disk during upgrades; the wizard never exposes them.
  llm?: { baseUrl: string; apiKey: string; modelId: string };
  deployment?: DeploymentTarget;
  apps: string[];
}

export interface DeploymentTarget {
  installation?: Installation;
  runner: 'native' | 'docker';
  root: string;
  environment: string;
  kubeconfig: string;
  workDir: string;
  image: string;
  offline: boolean;
  bundleDir: string;
}

export function deploymentDefaults(): DeploymentTarget {
  const installation = installationDefaults();
  return {
    installation,
    runner: installation.topology === 'single-k3d' ? 'docker' : 'native',
    root: '',
    environment: process.env.CHENTU_ENV || process.env.LAB_ENV || '',
    kubeconfig: process.env.KUBECONFIG || '', workDir: process.env.LAB_WORK_DIR || '',
    image: process.env.LAB_IMAGE || 'chentu-lab',
    offline: false, bundleDir: '',
  };
}

function absolutePath(value: unknown, label: string): string {
  const result = text(value, label, 4096);
  if (!isAbsolute(result)) throw new ConfigError(`${label} 必须是运行 CLI 的这台机器上的绝对路径。`);
  return resolve(result);
}

function deploymentTarget(value: unknown): DeploymentTarget {
  const input = object(value);
  if ('profile' in input) throw new ConfigError('profile 已移除，请使用 installation.topology 选择部署拓扑。');
  if ('CHENTU_PROFILE' in process.env) throw new ConfigError('CHENTU_PROFILE 已移除，请取消该环境变量并选择部署拓扑。');
  if (input.installation !== undefined) {
    const installation = validateInstallation(input.installation);
    if (typeof input.offline !== 'boolean') throw new ConfigError('请选择在线或离线部署。');
    if (input.offline && installation.publicAccess) throw new ConfigError('公网入口需要在线 DNS 和证书服务；离线部署请使用内网访问。');
    const bundleDir = input.bundleDir ? absolutePath(input.bundleDir, '安装包目录') : '';
    if (input.offline && !bundleDir) throw new ConfigError('离线部署请选择本机的安装包目录。');
    if (input.offline && installation.topology === 'single-k3d') throw new ConfigError('宸途 k3d 准备脚本需要联网；离线部署请选择 K3s。');
    return { installation, runner: installation.topology === 'single-k3d' ? 'docker' : 'native',
      // Fresh installs resolve code from the release/offline package, including
      // when migrating a saved config that once exposed a source override.
      root: '',
      environment: '', kubeconfig: '',
      workDir: '', image: 'chentu-lab', offline: input.offline, bundleDir };
  }
  if (input.runner !== 'native' && input.runner !== 'docker') throw new ConfigError('请选择部署方式。');
  const docker = input.runner === 'docker';
  if (input.offline !== undefined && typeof input.offline !== 'boolean') throw new ConfigError('离线部署选项必须为布尔值。');
  const offline = input.offline === true;
  const bundleDir = input.bundleDir === undefined || input.bundleDir === '' ? '' : absolutePath(input.bundleDir, '本地 bundle 目录');
  if (offline && !bundleDir) throw new ConfigError('离线部署请填写本地 bundle 目录。');
  const image = docker ? text(input.image, '工具箱镜像', 256) : '';
  if (docker && (!/^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*$/.test(image))) throw new ConfigError('工具箱镜像名称不正确。');
  return {
    runner: input.runner, root: input.root === undefined || input.root === '' ? '' : absolutePath(input.root, '本地宸途源码'),
    environment: absolutePath(input.environment, '环境 values 文件'),
    kubeconfig: docker ? '' : absolutePath(input.kubeconfig, 'Kubeconfig'),
    workDir: docker ? absolutePath(input.workDir, '工作目录') : '', image,
    offline, bundleDir,
  };
}

export class ConfigError extends Error {
  constructor(message: string, readonly status = 400) { super(message); }
}

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ConfigError('配置格式不正确。');
  return value as Record<string, unknown>;
}

function text(value: unknown, name: string, max: number, optional = false): string {
  if (typeof value !== 'string' || value.length > max || /[\x00-\x1f\x7f]/.test(value)) {
    throw new ConfigError(`${name} 格式不正确。`);
  }
  const result = value.trim();
  if (!optional && !result) throw new ConfigError(`请填写 ${name}。`);
  return result;
}

export function validateConfig(input: unknown, current?: Configuration): Configuration {
  const body = object(input);
  const slug = text(body.slug, 'Slug', 63);
  if (!/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(slug)) {
    throw new ConfigError('Slug 只能包含小写字母、数字和连字符，且不能以连字符开头或结尾。');
  }
  if (!Array.isArray(body.apps) || !body.apps.length || body.apps.length > APPS.length ||
      body.apps.some(id => typeof id !== 'string' || !APPS.some(app => app.id === id)) ||
      new Set(body.apps).size !== body.apps.length) {
    throw new ConfigError('请至少选择一个支持的应用，且不要重复选择。');
  }
  const requestedApps = body.apps;
  return {
    schemaVersion: 2, slug,
    ...(current?.llm ? { llm: current.llm } : {}),
    ...(body.deployment !== undefined ? { deployment: deploymentTarget(body.deployment) } : {}),
    apps: APPS.filter(a => requestedApps.includes(a.id)).map(a => a.id),
  };
}

export function defaultConfigPath(): string {
  const home = process.env.XDG_CONFIG_HOME || join(homedir(), '.config');
  if (!isAbsolute(home)) throw new ConfigError('XDG_CONFIG_HOME 必须是绝对路径。');
  return join(home, 'apeiron', 'config.json');
}

interface Snapshot { config?: Configuration; revision: string | null }

export class ConfigStore {
  private writing = false;
  constructor(readonly path: string) {}

  async checkLocation(): Promise<void> {
    let ancestor = dirname(this.path);
    for (;;) {
      try { ancestor = await realpath(ancestor); break; }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT' || dirname(ancestor) === ancestor) throw error;
        ancestor = dirname(ancestor);
      }
    }
    for (;;) {
      try { await lstat(join(ancestor, '.git')); throw new ConfigError('配置包含密钥，请把配置文件放在 Git 工作目录之外。'); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      if (dirname(ancestor) === ancestor) break;
      ancestor = dirname(ancestor);
    }
  }

  async read(): Promise<Snapshot> {
    let raw: string;
    try {
      const stat = await lstat(this.path);
      if (!stat.isFile() || stat.isSymbolicLink()) throw new ConfigError('配置路径必须是普通文件。', 409);
      if (stat.size > 16_384) throw new ConfigError('现有配置文件过大。', 409);
      raw = await readFile(this.path, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { revision: null };
      throw error;
    }
    try {
      const body = object(JSON.parse(raw));
      if (body.schemaVersion !== 1 && body.schemaVersion !== 2) throw new Error('version');
      const config = validateConfig(body);
      if (body.llm !== undefined) {
        const llm = object(body.llm);
        config.llm = { baseUrl: text(llm.baseUrl, 'Base URL', 2048), modelId: text(llm.modelId, 'Model ID', 256), apiKey: text(llm.apiKey, 'API Key', 4096, true) };
      }
      return { config, revision: createHash('sha256').update(raw).digest('hex') };
    } catch { throw new ConfigError('现有配置格式或版本不支持，已保留原文件。请改用其他 --config 路径。', 409); }
  }

  async save(input: unknown): Promise<Snapshot> {
    if (this.writing) throw new ConfigError('另一个保存操作正在进行，请稍后重试。', 409);
    this.writing = true;
    let temporary: string | undefined;
    let lock: Awaited<ReturnType<typeof open>> | undefined;
    try {
      await this.checkLocation();
      const body = object(input);
      const snapshot = await this.read();
      if (body.revision !== snapshot.revision) throw new ConfigError('配置已在其他窗口或进程中更新，请刷新后重试。', 409);
      const config = validateConfig(body, snapshot.config);
      if (!config.deployment) throw new ConfigError('请填写部署环境。');
      const missing = APPS.filter(app => app.required && !config.apps.includes(app.id));
      if (missing.length) throw new ConfigError(`以下应用为必选：${missing.map(app => app.name).join('、')}。`);
      const raw = JSON.stringify(config, null, 2) + '\n';
      await mkdir(dirname(this.path), { recursive: true, mode: 0o700 });
      try { lock = await open(this.path + '.lock', 'wx', 0o600); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'EEXIST') throw new ConfigError('配置正在被其他进程使用。若进程已退出，请检查配置旁的 .lock 文件。', 409);
        throw error;
      }
      if ((await this.read()).revision !== snapshot.revision) throw new ConfigError('配置已更改，请刷新后重试。', 409);
      temporary = this.path + '.' + randomBytes(8).toString('hex') + '.tmp';
      const file = await open(temporary, 'wx', 0o600);
      try { await file.writeFile(raw); await file.sync(); } finally { await file.close(); }
      await rename(temporary, this.path);
      temporary = undefined;
      return { config, revision: createHash('sha256').update(raw).digest('hex') };
    } finally {
      if (temporary) await unlink(temporary).catch(() => {});
      if (lock) { await lock.close(); await unlink(this.path + '.lock'); }
      this.writing = false;
    }
  }
}

export function publicSnapshot(snapshot: Snapshot) {
  return {
    revision: snapshot.revision,
    config: snapshot.config ? {
      slug: snapshot.config.slug, apps: snapshot.config.apps,
      deployment: snapshot.config.deployment,
    } : null,
  };
}

export function configPath(value?: string): string {
  const path = value ?? defaultConfigPath();
  if (/[\x00-\x1f\x7f]/.test(path)) throw new ConfigError('配置路径不能包含控制字符。');
  return resolve(path);
}
