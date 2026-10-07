import { chmod, mkdir, symlink } from 'node:fs/promises';
import { join } from 'node:path';
import { ConfigError } from './config';
import type { InstallPlan } from '../resources/install';
import type { RunCommand } from './bootstrap';

// Called only after every selected resource has passed checksum verification.
// Tools live in this installation's work directory; no global pip or PATH edits.
export async function prepareNativeTools(plan: InstallPlan, bundle: string, work: string,
  run: RunCommand, signal: AbortSignal): Promise<NodeJS.ProcessEnv> {
  const paths = new Set(plan.files.map(file => file.path));
  const python = [...paths].filter(path => /^python\/cpython-3\.12[.-][a-zA-Z0-9._-]+\.tar\.gz$/.test(path));
  const binaries = ['uv', 'helm', 'helmfile', 'age', 'age-keygen', 's5cmd'];
  if (python.length !== 1 || binaries.some(name => !paths.has(`bin/${name}`)) || !paths.has('k3s/k3s') ||
      ![...paths].some(path => /^cli\/ansible_core-[^/]+\.whl$/.test(path))) {
    throw new ConfigError('原生安装包缺少 Python、Ansible 或部署工具，请重新构建完整资源包。');
  }
  for (const path of [...binaries.map(name => `bin/${name}`), 'k3s/k3s']) await chmod(join(bundle, path), 0o700);
  const runtime = join(work, 'operator');
  await mkdir(runtime, { recursive: true, mode: 0o700 });
  const env = { ...process.env, UV_OFFLINE: '1', UV_PYTHON_DOWNLOADS: 'never',
    PATH: `${join(runtime, 'venv/bin')}:${join(bundle, 'bin')}:${process.env.PATH || ''}`,
    CHENTU_PYTHON: join(runtime, 'venv/bin/python3') };
  const checked = async (command: string, args: string[]) => {
    signal.throwIfAborted();
    if (await run(command, args, work, env) !== 0) throw new ConfigError('安装包内的部署工具初始化失败，请查看日志。尚未创建集群。');
  };
  await checked('tar', ['-xzf', join(bundle, python[0]!), '-C', runtime]);
  await checked(join(bundle, 'bin/uv'), ['venv', '--offline', '--python', join(runtime, 'python/bin/python3.12'), join(runtime, 'venv')]);
  await checked(join(bundle, 'bin/uv'), ['pip', 'install', '--offline', '--no-index', '--find-links', join(bundle, 'cli'),
    '--python', env.CHENTU_PYTHON, 'chentu', 'ansible-core']);
  // K3s supplies kubectl; use the exact verified binary before /usr/local/bin exists.
  await symlink(join(bundle, 'k3s/k3s'), join(runtime, 'venv/bin/kubectl'));
  await checked('ansible-playbook', ['--version']);
  await checked('helmfile', ['--version']);
  return env;
}
