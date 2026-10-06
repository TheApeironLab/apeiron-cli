import { expect, test } from 'bun:test';
import { appendFile, mkdtemp, readFile, rm, symlink, unlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { startInitServer } from '../src/init/server';
import { openDeploymentLog } from '../src/init/log';
import { installationDefaults } from '../src/init/installation';
import { deploymentFixture, freshInstallFixture, requiredApps } from './fixtures';

test('preparation failures and three retries each expose the current log, with token/origin and fixed-path protection', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-log-api-'));
  const setup = await deploymentFixture(directory);
  await freshInstallFixture(directory, setup, 0, 'logs.example.internal');
  const pathBefore = process.env.PATH; process.env.PATH = setup.bin + ':' + pathBefore;
  const server = await startInitServer({ path: join(directory, 'config.json'), resources: async () => directory });
  try {
    expect((await fetch(server.url + 'api/log')).status).toBe(404);
    let revision: string | null = null;
    const logs: string[] = [];
    for (let attempt = 0; attempt < 3; attempt++) {
      const response: Response = await fetch(server.url + 'api/deploy', { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision, slug: 'logs', apps: requiredApps, deployment: { offline: false,
          installation: { ...installationDefaults(), topology: 'single-k3d', domain: 'logs.example.internal', entryIp: '127.0.0.1' } } }) });
      expect(response.status).toBe(202);
      revision = (await response.json()).revision;
      for (let i = 0; i < 100 && server.result.phase !== 'failed'; i++) await Bun.sleep(10);
      expect(server.result.phase).toBe('failed');
      const logPath = server.result.log!;
      expect(logs).not.toContain(logPath);
      logs.push(logPath);
      const log = await fetch(server.url + 'api/log');
      expect(log.status).toBe(200);
      expect(log.headers.get('content-type')).toBe('text/plain; charset=utf-8');
      expect(log.headers.get('content-disposition')).toStartWith('inline;');
      expect(log.headers.get('cache-control')).toBe('no-store');
      expect(log.headers.get('x-content-type-options')).toBe('nosniff');
      const content = await log.text();
      expect(content).toBe(await readFile(logPath, 'utf8'));
      expect(content).toContain('开始安装检查。');
      expect(content).toContain('setup/install.json');
    }
    const downloaded = await fetch(server.url + 'api/log/download');
    expect(downloaded.headers.get('content-disposition')).toStartWith('attachment;');
    expect(await downloaded.text()).toBe(await readFile(logs[2]!, 'utf8'));
    expect((await fetch(server.origin + '/setup/wrong-token/api/log')).status).toBe(404);
    expect((await fetch(server.url + 'api/log?path=/etc/hosts')).status).toBe(404);
    expect((await fetch(server.url + 'api/log', { headers: { Origin: 'https://example.com' } })).status).toBe(403);
    expect((await fetch(server.url + 'api/log', { headers: { 'Sec-Fetch-Site': 'cross-site' } })).status).toBe(403);
    expect((await fetch(server.url + 'api/log', { headers: { Host: 'example.com' } })).status).toBe(403);
    await unlink(logs[2]!);
    await symlink(logs[0]!, logs[2]!);
    expect((await fetch(server.url + 'api/log')).status).toBe(404);
  } finally { await server.stop(); process.env.PATH = pathBefore; await rm(directory, { recursive: true, force: true }); }
});

test('log streams preserve UTF-8 and HTML as text, snapshot growing files, and handle cancellation', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-log-stream-'));
  const path = join(directory, 'install.log');
  try {
    const content = '部署日志 <script>alert(1)</script>\n'.repeat(5000);
    await writeFile(path, content);
    const log = await openDeploymentLog(path);
    await appendFile(path, 'later output');
    expect(await new Response(log.stream).text()).toBe(content);
    expect(log.size).toBe(Buffer.byteLength(content));
    const cancelled = await openDeploymentLog(path);
    await cancelled.stream.cancel();
    await expect(openDeploymentLog(directory)).rejects.toThrow('普通文件');
  } finally { await rm(directory, { recursive: true, force: true }); }
});
