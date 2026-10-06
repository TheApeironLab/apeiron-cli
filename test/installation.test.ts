import { expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { installationDefaults, inventoryFor, validateInstallation } from '../src/init/installation';
import { nodeArchitecture, sshArgs } from '../src/init/nodes';
import { validateConfig } from '../src/init/config';
import { installPlan, prepareInstallFiles, readInstallCatalog, type InstallCatalog } from '../src/resources/install';
import { Deployment } from '../src/init/deploy';
import { requiredApps } from './fixtures';
import { bootstrapFresh } from '../src/init/bootstrap';
import { startInitServer } from '../src/init/server';
import { resolveChentu } from '../src/resources/chentu';

const nodes = [0, 1, 2].map(i => ({ host: `node-${i}`, name: `node-${i}`, address: `192.0.2.${i + 10}`, role: i ? 'agent' as const : 'server' as const }));
const base = () => ({ ...installationDefaults(), topology: 'multi-k3s' as const, httpPort: 80, httpsPort: 443, domain: 'example.internal', entryIp: '192.0.2.10', nodes: nodes.slice(0, 2) });

test('topology is the only configuration selector; old profiles are rejected', () => {
  const input = { slug: 'team', apps: requiredApps, deployment: { installation: base(), offline: false } };
  expect(validateConfig(input).deployment).not.toHaveProperty('profile');
  expect(() => validateConfig({ ...input, deployment: { ...input.deployment, profile: 'ubuntu' } })).toThrow('profile 已移除');
  const previous = process.env.CHENTU_PROFILE;
  try {
    process.env.CHENTU_PROFILE = 'ubuntu';
    expect(() => validateConfig(input)).toThrow('CHENTU_PROFILE 已移除');
  } finally {
    if (previous === undefined) delete process.env.CHENTU_PROFILE;
    else process.env.CHENTU_PROFILE = previous;
  }
  expect(['x64', 'x86_64', 'amd64'].map(nodeArchitecture)).toEqual(['amd64', 'amd64', 'amd64']);
  expect(['arm64', 'aarch64'].map(nodeArchitecture)).toEqual(['arm64', 'arm64']);
  expect(() => nodeArchitecture('riscv64')).toThrow('不支持');
});

test('old packages and mismatched architecture fail before preparing resources', () => {
  const input = catalog();
  input.targets['k3s-amd64']!.deploymentTopology = false;
  expect(() => installPlan(input, 'k3s-amd64', [])).toThrow('deploymentTopology');
  input.targets['k3s-amd64']!.deploymentTopology = true;
  input.targets['k3s-amd64']!.environment = { profile: 'ubuntu' };
  expect(() => installPlan(input, 'k3s-amd64', [])).toThrow('profile');
  input.targets['k3s-amd64']!.environment = { architecture: 'arm64' };
  expect(() => installPlan(input, 'k3s-amd64', [])).toThrow('架构');
});

test('two-node installation is valid; only HA requires three odd-numbered control nodes', () => {
  expect(validateInstallation(base()).nodes).toHaveLength(2);
  expect(() => validateInstallation({ ...base(), ha: true })).toThrow('3 个');
  expect(validateInstallation({ ...base(), ha: true, nodes: nodes.map(node => ({ ...node, role: 'server' })) }).nodes).toHaveLength(3);
  expect(() => validateInstallation({ ...base(), nodes: nodes.slice(0, 2).map(node => ({ ...node, role: 'server' })) })).toThrow('1 个');
  expect(() => validateInstallation({ ...base(), nodes: [nodes[0], nodes[0]] })).toThrow('重复');
  expect(() => validateInstallation({ ...base(), topology: 'existing' })).toThrow();
  const inventory = inventoryFor(validateInstallation(base()), nodes.slice(0, 2), '/opt/bundle', '/tmp/kubeconfig', 'amd64');
  expect(Object.keys(inventory.all.children.server.hosts)).toEqual(['node-0']);
  expect(Object.keys(inventory.all.children.agent.hosts)).toEqual(['node-1']);
  expect(inventory.all.vars.ansible_ssh_common_args).toContain('StrictHostKeyChecking=yes');
});

test('rejects injection in SSH/inventory data and requires explicit offline package', () => {
  for (const host of ['-oProxyCommand=bad', 'test;echo', '{{lookup("pipe", "id")}}', 'user@host']) {
    expect(() => validateInstallation({ ...base(), nodes: [{ ...nodes[0], host }, nodes[1]] })).toThrow();
    expect(() => sshArgs(host, base())).toThrow();
  }
  expect(() => validateInstallation({ ...base(), sshKey: '/tmp/{{bad}}' })).toThrow();
  expect(() => validateInstallation({ ...base(), sshPort: 70000 })).toThrow();
  expect(() => validateConfig({ slug: 'team', apps: requiredApps, deployment: { installation: base(), offline: true } })).toThrow('安装包');
  expect(sshArgs('node-1', base())).toContain('python3 -');
});

const bytes = new TextEncoder().encode('fixture artifact\n');
function catalog(): InstallCatalog {
  return { schemaVersion: 1,
    targets: { 'k3s-amd64': { deploymentTopology: true, base: ['core'], environment: { releases: {} } } },
    components: { core: { requires: [], files: ['core.bin'] }, task: { requires: ['core'], files: ['task.bin'] }, optional: { requires: ['core'], files: ['optional.bin'] } },
    files: ['core.bin', 'task.bin', 'optional.bin'].map(path => ({ path, size: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), url: `https://downloads.example.internal/${path}` })),
  };
}

test('resource selection includes transitive dependencies but not unselected apps; missing and cyclic catalogs fail', () => {
  const input = catalog(); const plan = installPlan(input, 'k3s-amd64', ['task']);
  expect(plan.components).toEqual(['core', 'task']);
  expect(plan.files.map(file => file.path)).toEqual(['core.bin', 'task.bin']);
  expect(() => installPlan(input, 'k3s-arm64', ['task'])).toThrow('不支持');
  expect(() => installPlan(input, 'k3s-amd64', ['missing'])).toThrow('missing');
  input.components.core!.requires = ['task'];
  expect(() => installPlan(input, 'k3s-amd64', ['task'])).toThrow('循环');
});

test('image archives must be verified selected files and conflicting image versions are rejected', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-image-catalog-'));
  try {
    await mkdir(join(directory, 'setup'));
    const input = catalog();
    const image = { file: 'task.bin', reference: 'example/task:v1', digest: 'sha256:' + 'a'.repeat(64) };
    input.components.task!.images = [image];
    await writeFile(join(directory, 'setup/install.json'), JSON.stringify(input));
    const parsed = await readInstallCatalog(directory);
    expect(installPlan(parsed, 'k3s-amd64', ['task']).images).toEqual([image]);
    expect(installPlan(parsed, 'k3s-amd64', []).images).toEqual([]);
    input.components.core!.images = [{ ...image, file: 'core.bin', digest: 'sha256:' + 'b'.repeat(64) }];
    expect(() => installPlan(input, 'k3s-amd64', ['task'])).toThrow('冲突');
    input.components.task!.images = [{ ...image, file: 'optional.bin' }];
    await writeFile(join(directory, 'setup/install.json'), JSON.stringify(input));
    await expect(readInstallCatalog(directory)).rejects.toThrow('镜像文件');
    const target = input.targets['k3s-amd64']!;
    target.toolboxArchive = 'optional.bin'; target.toolboxImageId = 'sha256:' + 'a'.repeat(64);
    expect(() => installPlan(input, 'k3s-amd64', [])).toThrow('工具箱归档');
    target.toolboxArchive = 'core.bin';
    target.dockerArchives = [{ file: 'core.bin', images: [{ name: 'example/k3s:v1', id: 'latest' }] }];
    expect(() => installPlan(input, 'k3s-amd64', [])).toThrow('系统镜像');
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('404, truncated downloads and checksum failures never pass; offline makes zero requests', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-resource-check-'));
  let requests = 0;
  const transport = async () => { requests++; return new Response(bytes); };
  const plan = installPlan(catalog(), 'k3s-amd64', ['task']);
  const args = [plan, directory, false, new AbortController().signal, () => {}] as const;
  try {
    await expect(prepareInstallFiles(plan, directory, true, args[3], args[4], transport)).rejects.toThrow('离线');
    expect(requests).toBe(0);
    await expect(prepareInstallFiles(...args, async () => new Response('not found', { status: 404 }))).rejects.toThrow('HTTP 404');
    await expect(prepareInstallFiles(...args, async () => new Response('partial'))).rejects.toThrow('校验');
    await expect(prepareInstallFiles(...args, async () => new Response(new Uint8Array(bytes.length)))).rejects.toThrow('校验');
    await prepareInstallFiles(...args, transport);
    expect(requests).toBe(2);
    expect(await readFile(join(directory, 'core.bin'), 'utf8')).toBe('fixture artifact\n');
    await prepareInstallFiles(plan, directory, true, args[3], args[4], transport);
    expect(requests).toBe(2);
    await writeFile(join(directory, 'task.bin'), 'corrupted');
    await expect(prepareInstallFiles(plan, directory, true, args[3], args[4], transport)).rejects.toThrow('task.bin');
    expect(requests).toBe(2);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('release paths and symlinks cannot escape the selected package directory', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-resource-path-'));
  try {
    await mkdir(join(directory, 'setup'));
    const input = catalog(); input.files[0]!.path = '../escape';
    await writeFile(join(directory, 'setup/install.json'), JSON.stringify(input));
    await expect(readInstallCatalog(directory)).rejects.toThrow('安全路径');
    const plan = installPlan(catalog(), 'k3s-amd64', []);
    await symlink('/tmp', join(directory, 'linked'));
    await expect(prepareInstallFiles(plan, join(directory, 'linked'), true, new AbortController().signal, () => {})).rejects.toThrow('符号链接');
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('an unpublished/incomplete install package stops before running any cluster commands', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-install-gate-'));
  try {
    const deployment = new Deployment(join(directory, 'config.json'), undefined, async () => directory);
    await deployment.start(async () => validateConfig({ slug: 'team', apps: requiredApps, deployment: { installation: { ...base(), topology: 'single-k3d', entryIp: '127.0.0.1', nodes: [] }, offline: false } }));
    while (deployment.active) await Bun.sleep(10);
    expect(deployment.snapshot.phase).toBe('failed');
    expect(deployment.snapshot.message).toContain('setup/install.json');
    expect(deployment.snapshot.environment).toBeUndefined();
    const log = await readFile(deployment.snapshot.log!, 'utf8');
    expect(log).toContain('开始安装检查。');
    expect(log).toContain('setup/install.json');
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('native bootstrap validates configuration before Ansible, transfers packages before installing hosts', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-ansible-plan-'));
  const calls: { command: string; args: string[] }[] = [];
  const installation = validateInstallation(base());
  const target = validateConfig({ slug: 'team', apps: requiredApps, deployment: { installation, offline: true, bundleDir: directory } }).deployment!;
  Object.assign(target, { root: directory, workDir: join(directory, 'work'), kubeconfig: join(directory, 'kubeconfig') });
  const runtime = { architecture: 'amd64' as const, cluster: 'fixture', nodes: installation.nodes, bundle: directory, plan: installPlan(catalog(), 'k3s-amd64', ['task']) };
  const context = { directory, signal: new AbortController().signal, progress: () => {}, resources: async () => directory,
    run: async (command: string, args: string[]) => { calls.push({ command, args }); return 0; } };
  try {
    await bootstrapFresh(target, join(directory, 'environment.yaml'), context, runtime);
    expect(calls.map(call => call.command)).toEqual(['python3', 'ansible-playbook', 'ansible-playbook']);
    expect(calls[0]!.args[0]).toEndWith('/deploy/helmfile/scripts/check.py');
    expect(calls[1]!.args.at(-1)).toEndWith('/prepare-hosts.yaml');
    expect(calls[2]!.args.at(-1)).toEndWith('/bootstrap/hosts.yaml');
    const inventory = Bun.YAML.parse(await readFile(join(directory, 'inventory.yaml'), 'utf8')) as any;
    expect(Object.keys(inventory.all.children.server.hosts)).toEqual(['node-0']);
    expect(Object.keys(inventory.all.children.agent.hosts)).toEqual(['node-1']);
    expect(inventory.all.vars.chentu_architecture).toBe('amd64');
    const playbook = Bun.YAML.parse(await readFile(join(directory, 'prepare-hosts.yaml'), 'utf8')) as any;
    expect(playbook[0].tasks.map((task: { name: string }) => task.name)).toEqual(['Verify platform DNS on each node', 'Check peer SSH reachability', 'Create resource directories', 'Copy verified resources', 'Verify staged bundle on each node']);
    calls.length = 0;
    await expect(bootstrapFresh(target, '/tmp/environment.yaml', { ...context, run: async (command, args) => { calls.push({ command, args }); return 17; } }, runtime)).rejects.toThrow('退出码 17');
    expect(calls).toHaveLength(1);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('node endpoints reject cross-origin and invalid hosts without starting SSH', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-node-api-'));
  const server = await startInitServer({ path: join(directory, 'config.json') });
  try {
    const request = (origin: string, hosts: string[]) => fetch(server.url + 'api/nodes', {
      method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ hosts }),
    });
    expect((await request('https://untrusted.example', ['node-0'])).status).toBe(403);
    expect((await request(server.origin, ['-oProxyCommand=bad'])).status).toBe(400);
    expect((await request(server.origin, [])).status).toBe(400);
    expect(await Bun.file(join(directory, 'config.json')).exists()).toBe(false);
  } finally { await server.stop(); await rm(directory, { recursive: true, force: true }); }
});

test('offline installation uses bundled Chentu and cannot fall back to a stale source override', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'apeiron-bundled-chentu-'));
  const bundled = join(directory, 'chentu');
  const signal = new AbortController().signal;
  try {
    await expect(resolveChentu('/stale/developer/source', signal, () => {}, { offline: true, bundleDir: directory })).rejects.toThrow('离线安装包缺少');
    for (const path of ['setup/install.json', 'deploy/helmfile/run.sh', 'deploy/helmfile/helmfile.yaml.gotmpl', 'deploy/helmfile/scripts/check.py', 'cli/src/chentu/environment.py', 'tests/lab/helmfile.sh']) {
      const full = join(bundled, path);
      await mkdir(join(full, '..'), { recursive: true });
      await writeFile(full, 'fixture-only');
    }
    expect(await resolveChentu('/stale/developer/source', signal, () => {}, { offline: true, bundleDir: directory })).toBe(bundled);
    const config = validateConfig({ slug: 'team', apps: requiredApps, deployment: { installation: base(), root: '/stale/developer/source', offline: true, bundleDir: directory } });
    expect(config.deployment!.root).toBe('');
  } finally { await rm(directory, { recursive: true, force: true }); }
});


test('K3d public ports are validated and K3s keeps standard ports', () => {
  const local = { ...installationDefaults(), topology: 'single-k3d', domain: 'example.internal', entryIp: '127.0.0.1', nodes: [] };
  expect(validateInstallation({ ...local, httpPort: undefined, httpsPort: undefined })).toMatchObject({ httpPort: 54320, httpsPort: 54321 });
  expect(validateInstallation({ ...local, httpPort: 54322, httpsPort: 54323 })).toMatchObject({ httpPort: 54322, httpsPort: 54323 });
  for (const port of [0, 65536, 1.5, '54321', true]) expect(() => validateInstallation({ ...local, httpsPort: port })).toThrow();
  expect(() => validateInstallation({ ...local, httpPort: 54321, httpsPort: 54321 })).toThrow();
  expect(() => validateInstallation({ ...base(), httpsPort: 54321 })).toThrow('K3s');
});
