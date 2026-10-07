import { expect, test } from 'bun:test';
import { mkdtemp, rm, writeFile, mkdir, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { startInitServer } from './rust-server';
import { DeploymentClient, localInstallation } from './support';
import { requiredApps, deploymentFixture } from './fixtures';
import { tlsFixture } from './tls';
import { buildSupportWithEnv } from '../scripts/lib/rust';
test('node endpoints reject cross-origin and invalid hosts without starting SSH', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'apeiron-node-api-'));
    const server = await startInitServer({ path: join(directory, 'config.json') });
    try {
        const request = (origin, hosts) => fetch(server.url + 'api/nodes', {
            method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ hosts }),
        });
        expect((await request('https://untrusted.example', ['node-0'])).status).toBe(403);
        expect((await request(server.origin, ['-oProxyCommand=bad'])).status).toBe(400);
        expect((await request(server.origin, [])).status).toBe(400);
        expect(await Bun.file(join(directory, 'config.json')).exists()).toBe(false);
    }
    finally {
        await server.stop();
        await rm(directory, { recursive: true, force: true });
    }
});
test('404, truncated downloads and checksum failures never pass; offline makes zero requests', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'resources-http-'));
  const bundle = join(dir, 'bundle'); await mkdir(bundle);
  const bytes = Buffer.from('fixture artifact\n');
  let calls = 0, status = 200, body = bytes;
  const remote = await tlsFixture(dir, req => { calls++; expect(req.headers.has('authorization')).toBe(false); return new Response(body, { status }); });
  const plan = { files: ['core.bin', 'task.bin'].map(path => ({ path, size: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), url: remote.url + '/' + path })) };
  const file = join(dir, 'plan.json'); await writeFile(file, JSON.stringify(plan));
  const prepare = mode => buildSupportWithEnv({ SSL_CERT_FILE: remote.ca }, 'prepare-files', file, bundle, mode);
  try {
    await expect(prepare('offline')).rejects.toThrow('离线'); expect(calls).toBe(0);
    for (const code of [204, 206, 403, 404, 500]) {
      status = code; body = code === 204 ? null : 'not found';
      await expect(prepare('online')).rejects.toThrow('HTTP ' + code);
    }
    status = 200; body = 'partial'; await expect(prepare('online')).rejects.toThrow('校验');
    body = new Uint8Array(bytes.length); await expect(prepare('online')).rejects.toThrow('校验');
    body = bytes; await prepare('online');
    expect(await readFile(join(bundle, 'core.bin'))).toEqual(bytes);
    const downloaded = calls; await prepare('offline'); expect(calls).toBe(downloaded);
    await writeFile(join(bundle, 'task.bin'), 'tampered');
    await expect(prepare('offline')).rejects.toThrow('task.bin'); expect(calls).toBe(downloaded);
  } finally { remote.server.stop(true); await rm(dir, { recursive: true, force: true }); }
}, 30000);

test('an incomplete install package stops before any cluster command', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'incomplete-package-'));
  const setup = await deploymentFixture(directory);
  const deployment = new DeploymentClient(join(directory, 'config.json'), directory, { PATH: setup.bin + ':' + process.env.PATH });
  try {
    await deployment.start({ slug: 'example', apps: requiredApps, deployment: { offline: false, installation: localInstallation() } });
    while (deployment.active) await Bun.sleep(10);
    expect(deployment.snapshot.phase).toBe('failed');
    expect(deployment.snapshot.message).toContain('setup/install.json');
    expect(await Bun.file(setup.calls).exists()).toBe(false);
  } finally { await deployment.close(); await rm(directory, { recursive: true, force: true }); }
});
