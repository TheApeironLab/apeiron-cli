import { expect, test } from 'bun:test';
import { mkdtemp, mkdir, readlink, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { prepareNativeTools } from '../src/init/native-tools';
import type { InstallPlan } from '../src/resources/install';

test('native tools fail before execution when the verified resource set is incomplete', async () => {
  let calls = 0;
  const plan = { files: [] } as unknown as InstallPlan;
  await expect(prepareNativeTools(plan, '/missing', '/missing', async () => { calls++; return 0; }, new AbortController().signal)).rejects.toThrow('缺少');
  expect(calls).toBe(0);
});

test('native runtime uses isolated offline dependencies and propagates failures', async () => {
  const work = await mkdtemp(join(tmpdir(), 'apeiron-native-'));
  try {
    const bundle = join(work, 'bundle');
    const paths = ['bin/uv', 'bin/helm', 'bin/helmfile', 'bin/age', 'bin/age-keygen', 'bin/s5cmd', 'k3s/k3s',
      'python/cpython-3.12.14-aarch64.tar.gz', 'cli/ansible_core-2.19.3-py3-none-any.whl'];
    for (const path of paths) { await mkdir(dirname(join(bundle, path)), { recursive: true }); await writeFile(join(bundle, path), 'fixture', { mode: 0o600 }); }
    const plan = { files: paths.map(path => ({ path })) } as unknown as InstallPlan;
    const calls: { command: string; args: string[]; env: NodeJS.ProcessEnv }[] = [];
    const run = async (command: string, args: string[], cwd: string, env: NodeJS.ProcessEnv) => {
      calls.push({ command, args, env });
      await mkdir(join(work, 'operator/venv/bin'), { recursive: true });
      expect(cwd).toBe(work);
      return 0;
    };
    const env = await prepareNativeTools(plan, bundle, work, run, new AbortController().signal);
    expect(env.PATH?.split(':')[0]).toBe(join(work, 'operator/venv/bin'));
    expect(env.CHENTU_PYTHON).toBe(join(work, 'operator/venv/bin/python3'));
    expect(calls.every(call => call.env.UV_OFFLINE === '1' && call.env.UV_PYTHON_DOWNLOADS === 'never')).toBe(true);
    const install = calls.find(call => call.args[0] === 'pip')!;
    expect(install.args).toContain('--no-index');
    expect(install.args).toContain('--offline');
    expect(install.args).toContain(join(bundle, 'cli'));
    expect(await readlink(join(work, 'operator/venv/bin/kubectl'))).toBe(join(bundle, 'k3s/k3s'));
    expect((await stat(join(bundle, 'bin/helm'))).mode & 0o777).toBe(0o700);
    await expect(prepareNativeTools(plan, bundle, work, async () => 17, new AbortController().signal)).rejects.toThrow('初始化失败');
  } finally { await rm(work, { recursive: true, force: true }); }
});
