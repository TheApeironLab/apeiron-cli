import { createHash, randomBytes } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, mkdir, open, readFile, realpath, rename, rm } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { ConfigError } from '../init/config';
import { supportsK3sHost } from '../init/host-platform';

export interface InstallFile { path: string; size: number; sha256: string; url?: string }
export interface InstallImage { file: string; reference: string; digest: string }
export interface InstallComponent { requires: string[]; files: string[]; values?: Record<string, unknown>; images?: InstallImage[] }
export interface InstallTarget {
  operatorTools?: boolean;
  hostPlatform?: { os: 'ubuntu'; version: string; architecture: 'amd64' | 'arm64' };
  deploymentTopology?: boolean;
  publicPorts?: boolean;
  clusterOidc?: boolean;
  base: string[];
  environment: Record<string, unknown>;
  toolboxImage?: string;
  toolboxArchive?: string;
  toolboxImageId?: string;
  dockerArchives?: { file: string; images: { name: string; id: string }[] }[];
}
export interface InstallCatalog {
  schemaVersion: 1;
  targets: Record<string, InstallTarget>;
  components: Record<string, InstallComponent>;
  files: InstallFile[];
}
export interface InstallPlan { components: string[]; files: InstallFile[]; target: InstallTarget; environment: Record<string, unknown>; images: InstallImage[] }
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const safeRelative = (value: string) => /^[a-zA-Z0-9][a-zA-Z0-9._/-]*$/.test(value) && value.split('/').every(part => part && part !== '.' && part !== '..');

export async function readInstallCatalog(root: string): Promise<InstallCatalog> {
  let catalog: InstallCatalog;
  try {
    const file = join(root, 'setup/install.json');
    if ((await lstat(file)).size > 4 * 1024 * 1024) throw new Error();
    catalog = JSON.parse(await readFile(file, 'utf8'));
  } catch { throw new ConfigError('宸途安装包尚未提供 setup/install.json（应用版本、依赖及资源校验清单）。请使用支持全新安装的发行包；尚未修改任何集群。'); }
  if (!object(catalog) || catalog.schemaVersion !== 1 || !object(catalog.targets) || !object(catalog.components) || !Array.isArray(catalog.files)) throw new ConfigError('宸途安装资源清单格式不正确。');
  const paths = new Set<string>();
  for (const file of catalog.files) {
    if (!object(file) || typeof file.path !== 'string' || !safeRelative(file.path) || file.path === 'SHA256SUMS' || paths.has(file.path) ||
        !Number.isSafeInteger(file.size) || Number(file.size) <= 0 || typeof file.sha256 !== 'string' || !/^[a-f0-9]{64}$/.test(file.sha256)) throw new ConfigError('安装资源必须声明唯一安全路径、大小和 SHA-256。');
    paths.add(file.path);
    if (file.url !== undefined) {
      let url: URL;
      try { url = new URL(String(file.url)); } catch { throw new ConfigError('安装资源下载地址不正确。'); }
      if (url.protocol !== 'https:' || url.username || url.password) throw new ConfigError('安装资源必须使用 HTTPS 下载地址。');
    }
  }
  for (const [name, component] of Object.entries(catalog.components)) {
    if (!/^[a-z0-9-]+$/.test(name) || !object(component) || !Array.isArray(component.requires) || !Array.isArray(component.files) ||
        component.requires.some(dep => typeof dep !== 'string' || !catalog.components[dep]) || component.files.some(path => !paths.has(path)) ||
        (component.values !== undefined && !object(component.values))) throw new ConfigError('安装包的应用依赖或资源声明不完整。');
    if (component.images !== undefined && (!Array.isArray(component.images) || component.images.some(image =>
      !object(image) || typeof image.file !== 'string' || !component.files.includes(image.file) ||
      typeof image.reference !== 'string' || !/^[a-z0-9][a-z0-9._/-]*:[A-Za-z0-9._-]+$/.test(image.reference) ||
      typeof image.digest !== 'string' || !/^sha256:[a-f0-9]{64}$/.test(image.digest)))) throw new ConfigError('安装包的镜像文件或 digest 声明不完整。');
  }
  return catalog;
}

function merge(base: Record<string, unknown>, addition: Record<string, unknown>): Record<string, unknown> {
  const result = { ...base };
  for (const [key, value] of Object.entries(addition)) {
    if (['__proto__', 'prototype', 'constructor'].includes(key)) throw new ConfigError('安装包配置包含无效字段。');
    result[key] = object(value) && object(result[key]) ? merge(result[key], value) : value;
  }
  return result;
}

export function installPlan(catalog: InstallCatalog, target: string, apps: string[]): InstallPlan {
  const targetConfig = catalog.targets[target];
  if (!targetConfig || !Array.isArray(targetConfig.base) || !object(targetConfig.environment)) throw new ConfigError(`当前宸途发行包不支持 ${target}：缺少此目标的安装资源。请使用包含此目标的发行包；尚未修改任何集群。`);
  if (targetConfig.deploymentTopology !== true) throw new ConfigError('此安装包仍使用旧 profile 部署入口，请使用支持 deploymentTopology 的新版宸途发行包。');
  const visiting = new Set<string>(), done = new Set<string>();
  function visit(name: string) {
    if (done.has(name)) return;
    const component = catalog.components[name];
    if (!component) throw new ConfigError(`安装包缺少应用或依赖：${name}。`);
    if (visiting.has(name)) throw new ConfigError('安装包包含循环依赖。');
    visiting.add(name); component.requires.forEach(visit); visiting.delete(name); done.add(name);
  }
  if ("profile" in targetConfig.environment) throw new ConfigError("安装包包含已移除的 profile 配置，请使用新版宸途发行包。");
  [...targetConfig.base, ...apps].forEach(visit);
  const paths = new Set([...done].flatMap(name => catalog.components[name]!.files));
  const environment = [...done].reduce((values, name) => merge(values, catalog.components[name]!.values ?? {}), targetConfig.environment);
  if ('profile' in environment) throw new ConfigError('安装资源中不再支持 profile 字段。');
  const architecture = target.split('-').at(-1);
  if (environment.architecture !== undefined && environment.architecture !== '__ARCH__' && environment.architecture !== architecture) throw new ConfigError('安装包的资源架构与目标架构不一致。');
  const files = catalog.files.filter(file => paths.has(file.path));
  if (!files.length) throw new ConfigError('安装包未声明任何可校验的部署资源。');
  if (targetConfig.toolboxArchive && (!paths.has(targetConfig.toolboxArchive) || !/^sha256:[a-f0-9]{64}$/.test(targetConfig.toolboxImageId ?? ''))) throw new ConfigError('工具箱归档必须列入校验清单，并提供固定镜像 ID。');
  if (targetConfig.dockerArchives !== undefined && !Array.isArray(targetConfig.dockerArchives)) throw new ConfigError('K3d 系统镜像归档声明不完整。');
  for (const archive of targetConfig.dockerArchives ?? []) {
    if (!object(archive) || !paths.has(archive.file) || !Array.isArray(archive.images) || !archive.images.length || archive.images.some(image =>
      !object(image) || typeof image.name !== 'string' || !/^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*$/.test(image.name) || typeof image.id !== 'string' || !/^sha256:[a-f0-9]{64}$/.test(image.id))) throw new ConfigError('K3d 系统镜像归档声明不完整。');
  }
  const images = [...done].flatMap(name => catalog.components[name]!.images ?? []);
  const refs = new Map<string, InstallImage>();
  for (const image of images) {
    const prior = refs.get(image.reference);
    if (prior && (prior.digest !== image.digest || prior.file !== image.file)) throw new ConfigError('安装包包含冲突的镜像版本。');
    refs.set(image.reference, image);
  }
  return { components: [...done], files, target: targetConfig, environment, images: [...refs.values()] };
}

export function validateNativeHostPlatform(target: InstallTarget, hosts: { os: string; version: string; architecture: string }[]) {
  const platform = target.hostPlatform;
  if (!platform || platform.os !== 'ubuntu' || !['22.04', '24.04'].includes(platform.version) ||
      !['amd64', 'arm64'].includes(platform.architecture)) {
    throw new ConfigError('宸途原生 K3s 安装包未声明 hostPlatform（Ubuntu 版本与 CPU 架构），无法确认系统包是否匹配；尚未修改任何集群。');
  }
  for (const host of hosts) {
    const architecture = ['x64', 'x86_64', 'amd64'].includes(host.architecture) ? 'amd64' : host.architecture === 'aarch64' ? 'arm64' : host.architecture;
    if (!supportsK3sHost(host.os, host.version, host.architecture) || host.version.split('.').slice(0, 2).join('.') !== platform.version || architecture !== platform.architecture) {
      throw new ConfigError(`当前宸途安装包适用于 Ubuntu ${platform.version} / ${platform.architecture}，与节点 ${host.os} ${host.version} / ${host.architecture} 不匹配。请使用匹配的发行包；尚未修改任何集群。`);
    }
  }
}

async function safeDestination(base: string, path: string) {
  if (!safeRelative(path)) throw new ConfigError('安装资源路径不正确。');
  // Canonicalize OS aliases such as macOS /var -> /private/var. Reject a
  // symlink at the selected package root or inside it, not system ancestors.
  async function canonical(directory: string): Promise<string> {
    try { return await realpath(directory); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
      return join(await canonical(dirname(directory)), basename(directory));
    }
  }
  try { if ((await lstat(base)).isSymbolicLink()) throw new ConfigError('安装包目录不能包含符号链接。'); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
  let current = await canonical(resolve(base));
  const absolute = join(current, path);
  const parts = path.split('/');
  for (const [index, part] of parts.entries()) {
    current = join(current, part);
    try {
      const stat = await lstat(current);
      if (stat.isSymbolicLink() || (index < parts.length - 1 && !stat.isDirectory())) throw new ConfigError('安装包目录不能包含符号链接。');
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
  }
  return absolute;
}

async function validFile(path: string, file: InstallFile, signal: AbortSignal) {
  try {
    const stat = await lstat(path);
    if (!stat.isFile() || stat.size !== file.size) return false;
    const hash = createHash('sha256');
    for await (const chunk of createReadStream(path, { signal })) hash.update(chunk);
    return hash.digest('hex') === file.sha256;
  } catch { signal.throwIfAborted(); return false; }
}

type Fetch = (url: string, init: RequestInit) => Promise<Response>;
async function download(url: string, signal: AbortSignal, transport: Fetch) {
  for (let redirects = 0; redirects < 6; redirects++) {
    const parsed = new URL(url);
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password) throw new ConfigError('资源下载重定向必须使用 HTTPS。');
    const response = await transport(url, { method: 'GET', signal, redirect: 'manual' });
    if (![301, 302, 303, 307, 308].includes(response.status)) return response;
    const location = response.headers.get('location'); await response.body?.cancel();
    if (!location) break;
    url = new URL(location, url).href;
  }
  throw new ConfigError('资源下载重定向次数过多。');
}
export async function prepareInstallFiles(plan: InstallPlan, directory: string, offline: boolean, signal: AbortSignal,
  progress: (message: string) => void, transport: Fetch = fetch) {
  const missing: InstallFile[] = [];
  for (const file of plan.files) {
    signal.throwIfAborted();
    if (!await validFile(await safeDestination(directory, file.path), file, signal)) missing.push(file);
  }
  if (offline && missing.length) throw new ConfigError(`离线安装包缺少或校验失败：${missing.slice(0, 8).map(file => file.path).join('、')}${missing.length > 8 ? ` 等 ${missing.length} 项` : ''}。未发起网络请求。`);
  for (const [index, file] of missing.entries()) {
    signal.throwIfAborted();
    if (!file.url) throw new ConfigError(`资源 ${file.path} 不在本地，发行包未提供下载地址。`);
    progress(`准备资源 ${index + 1}/${missing.length}：${file.path}`);
    const destination = await safeDestination(directory, file.path);
    await mkdir(dirname(destination), { recursive: true, mode: 0o700 });
    const temporary = destination + '.' + randomBytes(8).toString('hex') + '.part';
    const output = await open(temporary, 'wx', 0o600);
    try {
      const response = await download(file.url, AbortSignal.any([signal, AbortSignal.timeout(30 * 60_000)]), transport);
      if (response.status !== 200 || !response.body) {
        await response.body?.cancel();
        throw new ConfigError(`资源下载失败：${file.path}（HTTP ${response.status}）。`);
      }
      const reader = response.body.getReader(), hash = createHash('sha256');
      let size = 0;
      try {
        for (;;) {
          const { done, value } = await reader.read(); if (done) break;
          size += value.length;
          if (size > file.size) throw new ConfigError(`资源大小不符：${file.path}。`);
          hash.update(value);
          let offset = 0;
          while (offset < value.length) offset += (await output.write(value, offset)).bytesWritten;
        }
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      if (size !== file.size || hash.digest('hex') !== file.sha256) throw new ConfigError(`资源完整性校验失败：${file.path}。`);
      await output.sync(); await output.close(); await rename(temporary, destination);
    } catch (error) {
      signal.throwIfAborted();
      if (error instanceof ConfigError) throw error;
      throw new ConfigError(`无法下载资源 ${file.path}，请检查下载源、权限或网络。`);
    } finally { await output.close().catch(() => {}); await rm(temporary, { force: true }); }
  }
  progress(`资源检查通过：${plan.components.length} 个组件，${plan.files.length} 个文件均已完整校验。`);
}
