import { createHash } from 'node:crypto';
import { lstat, mkdir, mkdtemp, open, readFile, rename, rm, unlink, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { isAbsolute, join } from 'node:path';
import { ConfigError, ConfigStore } from '../init/config';

// Pinned OSS branch preview. Update only after validating the published bytes.
// The deployment package excludes images, tools, dependencies and credentials.
// rc.4 adds public port propagation; verified resource archives remain pinned separately.
export const CHENTU_RELEASE = {
  version: '0.1.0-rc.4',
  revision: '3e297990e097c4463fd39ee5f41a8d62e3329e55',
  url: 'https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com/chentu/releases/0.1.0-rc.4/chentu-0.1.0-rc.4.tar.gz',
  sha256: '393d0c6aa09970d1a69fe32579d46b4255296798adbb4c3fbf4a7dde0c676221',
} as const;

export type Release = { version: string; revision: string; url: string; sha256: string };
type Progress = (message: string) => void;
export type ResourceOptions = { offline: boolean; bundleDir: string };
export type ResourceResolver = (override: string, signal: AbortSignal, progress: Progress, options: ResourceOptions) => Promise<string>;
const hash = (data: Uint8Array) => createHash('sha256').update(data).digest('hex');
const maxArchiveSize = 64 * 1024 * 1024;
const entrypoints = ['deploy/helmfile/run.sh', 'deploy/helmfile/helmfile.yaml.gotmpl', 'deploy/helmfile/scripts/check.py', 'cli/src/chentu/environment.py', 'tests/lab/helmfile.sh'];

export function chentuCachePath(): string {
  const base = process.env.XDG_CACHE_HOME || join(homedir(), '.cache');
  if (!isAbsolute(base)) throw new ConfigError('XDG_CACHE_HOME 必须是绝对路径。');
  return join(base, 'apeiron', 'chentu');
}

function trustedUrl(url: string): boolean {
  try {
    const parsed = new URL(url);
    return parsed.protocol === 'https:' && !parsed.username && !parsed.password && !parsed.port &&
      !parsed.search && !parsed.hash && parsed.hostname === 'apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com' &&
      parsed.pathname.startsWith('/chentu/releases/');
  } catch { return false; }
}

export async function downloadChentu(release: Release, signal: AbortSignal,
  transport: (url: string, init: RequestInit) => Promise<Response> = fetch): Promise<Uint8Array> {
  let url = release.url;
  const timeout = AbortSignal.timeout(120_000);
  const combined = AbortSignal.any([signal, timeout]);
  try {
    for (let redirects = 0; redirects < 5; redirects++) {
      if (!trustedUrl(url)) {
        throw new ConfigError('宸途下载地址不受信任，已停止下载。');
      }
      const response = await transport(url, { redirect: 'manual', signal: combined, headers: {
        'User-Agent': 'apeiron-cli', Accept: 'application/octet-stream',
      } });
      if ([301, 302, 303, 307, 308].includes(response.status)) {
        const location = response.headers.get('location');
        await response.body?.cancel();
        if (!location) break;
        url = new URL(location, url).href;
        continue;
      }
      if (response.status !== 200 || !response.body) {
        await response.body?.cancel();
        throw new ConfigError(`宸途 OSS 发行包下载失败（HTTP ${response.status}），请检查网络或发行包访问权限后重试。`);
      }
      const reader = response.body.getReader();
      const chunks: Uint8Array[] = [];
      let size = 0;
      try {
        for (;;) {
          const chunk = await reader.read();
          if (chunk.done) break;
          size += chunk.value.length;
          if (size > maxArchiveSize) throw new ConfigError('宸途资源包超过大小限制，已停止下载。');
          chunks.push(chunk.value);
        }
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      return Buffer.concat(chunks);
    }
    throw new ConfigError('宸途资源下载重定向异常。');
  } catch (error) {
    signal.throwIfAborted();
    if (error instanceof ConfigError) throw error;
    // Fetch errors may include signed URLs or auth headers. Never expose them.
    throw new ConfigError('无法下载宸途资源，请检查网络连接后重试，或使用已验证的离线缓存。');
  }
}

export class ChentuResources {
  constructor(
    private readonly cache: string,
    private readonly release: Release = CHENTU_RELEASE,
    private readonly download: typeof downloadChentu = downloadChentu,
  ) {}

  async resolve(signal: AbortSignal, progress: Progress, options: ResourceOptions = { offline: false, bundleDir: '' }): Promise<string> {
    if (!/^[a-f0-9]{40}$/.test(this.release.revision) || !/^[a-f0-9]{64}$/.test(this.release.sha256) ||
        !/^\d+\.\d+\.\d+(?:-rc\.\d+)?$/.test(this.release.version) || !trustedUrl(this.release.url)) throw new ConfigError('宸途安装包版本声明无效。');
    await new ConfigStore(join(this.cache, 'guard')).checkLocation();
    await mkdir(this.cache, { recursive: true, mode: 0o700 });
    const id = 'chentu-' + this.release.version;
    const archivePath = join(this.cache, id + '.tar.gz');
    const destination = join(this.cache, id);
    const lockPath = join(this.cache, id + '.lock');
    const deadline = Date.now() + 180_000;
    let lock: Awaited<ReturnType<typeof open>>;
    for (;;) {
      signal.throwIfAborted();
      try { lock = await open(lockPath, 'wx', 0o600); break; }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
        if (Date.now() >= deadline) throw new ConfigError('宸途资源正被其他进程准备，请稍后重试。若进程已退出，请检查缓存目录中的 .lock 文件。', 409);
        await new Promise(resolve => setTimeout(resolve, 200));
      }
    }
    let staging: string | undefined;
    try {
      signal.throwIfAborted();
      let bytes: Uint8Array | undefined;
      try {
        const info = await lstat(archivePath);
        if (info.isFile() && info.size <= maxArchiveSize) {
          const cached = await readFile(archivePath);
          if (hash(cached) === this.release.sha256) bytes = cached;
        }
      } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      staging = await mkdtemp(join(this.cache, '.prepare-'));
      if (!bytes && options.bundleDir) {
        const supplied = join(options.bundleDir, id + '.tar.gz');
        try {
          const info = await lstat(supplied);
          if (!info.isFile() || info.size > maxArchiveSize) throw new ConfigError('本地宸途安装包格式或大小不正确。');
          bytes = await readFile(supplied);
          if (hash(bytes) !== this.release.sha256) throw new ConfigError('本地宸途安装包校验失败，请提供此 CLI 对应的安装包。');
        } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      }
      if (!bytes) {
        if (options.offline) throw new ConfigError(`离线部署缺少宸途安装包 ${id}.tar.gz。请将它放入本地 bundle 目录后重试；未发起网络请求。`);
        progress(`正在下载宸途 ${this.release.version} 安装包…`);
        bytes = await this.download(this.release, signal);
        signal.throwIfAborted();
        if (bytes.length > maxArchiveSize || hash(bytes) !== this.release.sha256) throw new ConfigError('宸途资源包校验失败，已停止部署。请重试或检查离线资源包版本。');
        const temporary = join(staging, 'archive.tar.gz');
        await writeFile(temporary, bytes, { mode: 0o600, flag: 'wx' });
        await rename(temporary, archivePath);
      }
      progress('正在校验宸途部署资源…');
      const archive = new Bun.Archive(bytes);
      const files = await archive.files();
      const prefix = id;
      // The archive's digest is trusted; additionally constrain its source paths.
      for (const path of files.keys()) {
        if (!path.startsWith(prefix + '/') || path.split('/').some(part => part === '..' || part === '.') || path.includes('\\')) {
          throw new ConfigError('宸途资源包目录结构不正确。');
        }
      }
      for (const path of entrypoints) if (!files.has(prefix + '/' + path)) throw new ConfigError('宸途资源包缺少部署入口。');
      let manifest: { schemaVersion?: number; kind?: string; version?: string; revision?: string };
      try { manifest = JSON.parse(await files.get(prefix + '/chentu-package.json')!.text()); }
      catch { throw new ConfigError('宸途安装包缺少有效的版本清单。'); }
      if (manifest?.schemaVersion !== 1 || manifest.kind !== 'chentu-deployment' || manifest.version !== this.release.version || manifest.revision !== this.release.revision) {
        throw new ConfigError('宸途安装包版本与 CLI 要求不符。');
      }
      const root = join(destination, prefix);
      if (await this.intact(destination, files, signal)) {
        progress('已复用本机验证通过的宸途部署资源。');
        return root;
      }
      signal.throwIfAborted();
      const extracted = join(staging, 'extracted');
      await archive.extract(extracted);
      if (!await this.intact(extracted, files, signal)) throw new ConfigError('宸途资源解包校验失败。');
      // Replace only the managed version directory, never a developer checkout.
      try { await rename(destination, join(staging, 'previous')); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      await rename(extracted, destination);
      progress('宸途部署资源已就绪。');
      return root;
    } finally {
      try { if (staging) await rm(staging, { recursive: true, force: true }); }
      finally { await lock.close(); await unlink(lockPath); }
    }
  }

  private async intact(base: string, files: Map<string, File>, signal: AbortSignal): Promise<boolean> {
    try {
      if (!(await lstat(base)).isDirectory()) return false;
      const entries = [...files];
      for (let i = 0; i < entries.length; i += 24) {
        signal.throwIfAborted();
        const valid = await Promise.all(entries.slice(i, i + 24).map(async ([path, original]) => {
          const target = join(base, path);
          const info = await lstat(target);
          if (!info.isFile() || info.size !== original.size) return false;
          return hash(await readFile(target)) === hash(new Uint8Array(await original.arrayBuffer()));
        }));
        if (valid.includes(false)) return false;
      }
      return true;
    } catch { signal.throwIfAborted(); return false; }
  }
}

export const resolveChentu: ResourceResolver = async (override, signal, progress, options) => {
  // Offline installs always use the chosen package, even if the operator has
  // developer overrides in their shell or an older saved configuration.
  if (options.bundleDir) {
    const bundled = join(options.bundleDir, 'chentu');
    if (await Bun.file(join(bundled, 'setup/install.json')).exists()) {
      for (const path of entrypoints) if (!await Bun.file(join(bundled, path)).exists()) throw new ConfigError('离线安装包缺少宸途部署入口。');
      progress('使用所选安装包中的宸途部署程序。');
      return bundled;
    }
  }
  if (options.offline) throw new ConfigError('离线安装包缺少 chentu/ 部署程序或 setup/install.json 资源清单。请选择完整安装包；未发起网络请求。');
  // Contributor-only process override; never accepted from the fresh-install UI
  // and never used in offline mode. Also supports isolated compiled-CLI tests.
  const developmentRoot = override || process.env.APEIRON_CHENTU_ROOT;
  if (developmentRoot) { progress('使用显式配置的开发部署程序。'); return developmentRoot; }
  return new ChentuResources(chentuCachePath()).resolve(signal, progress, options);
};
