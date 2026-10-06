import { createHash, randomBytes } from 'node:crypto';
import { lstat, mkdir, open, readFile, realpath, rename, unlink } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, resolve } from 'node:path';

// IDs match Chentu releases (Grafana is provided by kps). Deployment dependencies
// remain owned by Helmfile; this is only the user's requested application set.
export const APPS = [
  { id: 'vasi', name: 'Vasi', description: '集群资源与 Kubernetes 管理', selected: true, required: true },
  { id: 'apeiron', name: 'Apeiron', description: 'AI 工作台与平台管理（含 Ops）', selected: true, required: true },
  { id: 'ontology', name: 'Limani', description: '本体与业务建模', selected: true, required: true },
  { id: 'task', name: 'Task', description: '项目与任务协作', selected: true, required: true },
  { id: 'corpus', name: 'Corpus', description: '文档、知识检索与团队资料', selected: true, required: true },
  { id: 'matrix', name: 'Chat', description: '团队消息与房间协作', selected: true, required: true },
  { id: 'files', name: 'Files', description: '私人文件与团队共享文件', selected: true, required: false },
  { id: 'gateway', name: 'Gateway', description: '模型、访问凭证与用量管理', selected: true, required: false },
  { id: 'nexus', name: 'Nexus', description: '制品与镜像仓库', selected: true, required: false },
  { id: 'filer', name: 'Filer', description: '对象存储管理', selected: false, required: false },
  { id: 'stalwart', name: '邮件', description: '收件箱与邮件收发', selected: false, required: false },
  { id: 'git', name: '代码仓库', description: 'Git 代码托管与代码评审', selected: false, required: false },
  { id: 'gpustack', name: 'GPUStack', description: 'GPU 模型部署与推理服务管理', selected: false, required: false },
  { id: 'langfuse', name: 'Langfuse', description: '模型调用追踪、评测与提示词管理', selected: false, required: false },
  { id: 'kps', name: 'Grafana', description: '监控仪表盘与运行指标', selected: false, required: false },
] as const;

export interface Configuration {
  schemaVersion: 1;
  slug: string;
  llm: { baseUrl: string; apiKey: string; modelId: string };
  apps: string[];
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
  const llm = object(body.llm);
  const baseUrl = text(llm.baseUrl, 'Base URL', 2048);
  let url: URL;
  try { url = new URL(baseUrl); } catch { throw new ConfigError('请填写完整的 HTTP 或 HTTPS Base URL。'); }
  if (!['http:', 'https:'].includes(url.protocol) || !url.hostname || url.username || url.password || url.search || url.hash) {
    throw new ConfigError('Base URL 需使用 HTTP/HTTPS，且不能包含凭据、查询参数或片段。');
  }
  const modelId = text(llm.modelId, 'Model ID', 256);
  const apiKey = llm.apiKey === undefined ? current?.llm.apiKey ?? '' : text(llm.apiKey, 'API Key', 4096, true);
  if (!Array.isArray(body.apps) || !body.apps.length || body.apps.length > APPS.length ||
      body.apps.some(id => typeof id !== 'string' || !APPS.some(app => app.id === id)) ||
      new Set(body.apps).size !== body.apps.length) {
    throw new ConfigError('请至少选择一个支持的应用，且不要重复选择。');
  }
  const requestedApps = body.apps;
  return { schemaVersion: 1, slug, llm: { baseUrl, apiKey, modelId }, apps: APPS.filter(a => requestedApps.includes(a.id)).map(a => a.id) };
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
      if (body.schemaVersion !== 1) throw new Error('version');
      return { config: validateConfig(body), revision: createHash('sha256').update(raw).digest('hex') };
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
      llm: { baseUrl: snapshot.config.llm.baseUrl, modelId: snapshot.config.llm.modelId, hasApiKey: Boolean(snapshot.config.llm.apiKey) },
    } : null,
  };
}

export function configPath(value?: string): string {
  const path = value ?? defaultConfigPath();
  if (/[\x00-\x1f\x7f]/.test(path)) throw new ConfigError('配置路径不能包含控制字符。');
  return resolve(path);
}
