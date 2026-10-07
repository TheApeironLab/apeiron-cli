import { expect, test } from 'bun:test';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { installationDefaults, validateInstallation } from '../src/init/installation';
import { validateConfig, type DeploymentTarget } from '../src/init/config';
import { checkDns } from '../src/init/dns';
import { collectAccess } from '../src/init/access';
import { publicAccessPhase } from '../src/init/public-access';

const installation = () => ({ ...installationDefaults(), topology: 'single-k3s' as const,
  domain: 'team.example.com', entryIp: '192.168.1.10', httpPort: 80, httpsPort: 443,
  publicAccess: { mode: 'relay' as const, publicIp: '8.8.8.8', tunnelPort: 19444,
    gateway: { host: 'edge-host', sshUser: 'root', sshKey: '', sshPort: 22 } } });

test('public edge keeps origin IP separate and rejects private addresses, reserved names and shell input', () => {
  expect(validateInstallation(installation()).publicAccess?.publicIp).toBe('8.8.8.8');
  for (const ip of ['127.0.0.1', '10.0.0.1', '172.16.1.1', '192.168.1.1', '100.64.0.1']) {
    expect(() => validateInstallation({ ...installation(), publicAccess: { ...installation().publicAccess, publicIp: ip } })).toThrow('公网入口 IPv4');
  }
  for (const domain of ['team.internal', 'team.local', 'team.test']) expect(() => validateInstallation({ ...installation(), domain })).toThrow('真实域名');
  expect(() => validateInstallation({ ...installation(), topology: 'single-k3d' })).toThrow('单机 K3s');
  expect(() => validateInstallation({ ...installation(), publicAccess: { ...installation().publicAccess, gateway: { ...installation().publicAccess.gateway, host: 'host;id' } } })).toThrow('SSH');
});

test('offline setup never claims to provision an online public edge', () => {
  expect(() => validateConfig({ slug: 'team', apps: ['apeiron'], deployment: { installation: installation(), offline: true, bundleDir: '/media/bundle' } })).toThrow('离线部署请使用内网');
});

test('public access does not ask the browser to install a private CA or hosts', async () => {
  const result = await collectAccess({ installation: installation() } as DeploymentTarget, '/unused', new AbortController().signal);
  expect(result.info.public).toBe(true);
  expect(result.info.entryIp).toBe('8.8.8.8');
  expect(result.ca).toBeUndefined();
  expect(result.hosts).toBeUndefined();
});

test('unsupported release refuses public access before executing host commands', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-public-'));
  try {
    const calls: string[] = [];
    const target = { installation: installation(), root: directory } as DeploymentTarget;
    await expect(publicAccessPhase('check', target, directory, process.env, async command => { calls.push(command); return 0; })).rejects.toThrow('尚未包含');
    expect(calls).toEqual([]);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('public verification failure propagates so setup cannot report success', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-public-'));
  try {
    await mkdir(join(directory, 'bootstrap'));
    await Bun.write(join(directory, 'bootstrap/public_access.py'), '# fixture');
    const target = { installation: installation(), root: directory, workDir: directory, kubeconfig: '/tmp/kubeconfig' } as DeploymentTarget;
    await expect(publicAccessPhase('finish', target, directory, process.env, async () => 1)).rejects.toThrow('HTTPS 验证');
    const plan = await Bun.file(join(directory, 'public-access.json')).json();
    expect(plan.access.mode).toBe('relay');
    expect(plan.ca).toBe(join(directory, 'state/chentu-ca.crt'));
  } finally { await rm(directory, { recursive: true, force: true }); }
});


test('public DNS checks include the root used by Matrix discovery and remain cancellable', async () => {
  const names: string[] = [];
  const result = await checkDns({ domain: 'team.example.com', entryIp: '8.8.8.8', local: false }, {
    includeRoot: true, lookup: async host => { names.push(host); return [{ address: '8.8.8.8' }]; },
  });
  expect(result.passed).toBe(true);
  expect(names).toContain('team.example.com');
  expect(names.length).toBe(4);
  const timeout = await checkDns({ domain: 'team.example.com', entryIp: '8.8.8.8', local: false }, {
    includeRoot: true, timeoutMs: 10, lookup: () => new Promise(() => {}),
  });
  expect(timeout.checks.every(check => check.status === 'timeout')).toBe(true);
});
