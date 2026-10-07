import { createHash } from 'node:crypto';
import { copyFile, mkdir, writeFile } from 'node:fs/promises';
import { constants } from 'node:fs';
import { dirname, join } from 'node:path';
import { ConfigError, type Configuration, type DeploymentTarget } from './config';
import { inventoryFor, type NodeTarget } from './installation';
import { nodeArchitecture, probeNode, probeNodes, type NodeFacts } from './nodes';
import { installPlan, prepareInstallFiles, readInstallCatalog, validateNativeHostPlatform } from '../resources/install';
import type { ResourceResolver } from '../resources/chentu';
import { checkDns } from './dns';
import { safeEntryIp } from './installation';
import { checkLocalCluster, localCluster } from './local-cluster';
export { checkLocalPort } from './local-cluster';

export type RunCommand = (command: string, args: string[], cwd: string, env: NodeJS.ProcessEnv) => Promise<number>;
type Context = { directory: string; installationKey?: string; signal: AbortSignal; progress: (message: string) => void; run: RunCommand; resources: ResourceResolver };
const hash = (text: string) => createHash('sha256').update(text).digest('hex').slice(0, 12);

export async function prepareFreshInstallation(config: Configuration, context: Context): Promise<{ target: DeploymentTarget; source: string; runtime: Runtime }> {
  const { signal, progress, directory } = context;
  const target = { ...config.deployment! }, installation = target.installation!;
  const docker = installation.topology === 'single-k3d';
  if (!safeEntryIp(installation.entryIp)) throw new ConfigError('请返回部署环境，确认平台入口 IP。');
  const nodes: NodeTarget[] = [...installation.nodes];
  // Resolve the release contract before touching target machines.
  target.root = await context.resources(target.root, signal, progress, target);
  const catalog = await readInstallCatalog(target.root);
  let architecture = nodeArchitecture(process.arch);
  let facts: NodeFacts[] = [];
  if (!docker) {
    progress('复查节点系统、架构、权限和已有集群。');
    facts = installation.topology === 'multi-k3s'
      ? await probeNodes({ ...installation, hosts: nodes.map(node => node.host) }, signal)
      : [await probeNode('localhost', installation, signal, true)];
    for (const node of facts) if (node.error) throw new ConfigError(`${node.host}：${node.error}`);
    const architectures = new Set(facts.map(node => nodeArchitecture(node.architecture)));
    if (architectures.size !== 1) throw new ConfigError('当前安装包按单一架构发布，请选择 CPU 架构一致的节点。');
    architecture = nodeArchitecture(facts[0]!.architecture);
    if (installation.topology === 'single-k3s') nodes.push({ host: 'localhost', name: facts[0]!.name, address: facts[0]!.addresses[0]!, role: 'server' });
    for (const node of nodes) if (!facts.find(fact => fact.host === node.host)?.addresses.includes(node.address)) throw new ConfigError(`${node.host} 的内网地址已变化，请重新检测。`);
  }
  const plan = installPlan(catalog, `${docker ? 'k3d' : 'k3s'}-${architecture}`, config.apps);
  if (!docker) validateNativeHostPlatform(plan.target, facts);
  if (config.apps.includes('vasi') && (plan.target.clusterOidc !== true || !plan.components.includes('cluster-access') ||
      !await Bun.file(join(target.root, 'cli/src/chentu/cluster_oidc.py')).exists() ||
      !await Bun.file(join(target.root, 'bootstrap/oidc.yaml')).exists())) {
    throw new ConfigError('此宸途安装包未包含完整的集群 SSO 初始化，请使用支持 clusterOidc 的新版发行包。尚未修改任何集群。');
  }
  if (docker && (installation.httpPort !== 80 || installation.httpsPort !== 443) && plan.target.publicPorts !== true) throw new ConfigError('此宸途安装包不支持自定义入口端口，请使用新版发行包。尚未创建集群。');
  for (const tool of docker ? ['docker'] : ['ansible-playbook', 'python3', 'helm', 'helmfile', ...(installation.topology === 'multi-k3s' ? ['ssh'] : [])]) {
    if (!Bun.which(tool)) throw new ConfigError(`管理机缺少 ${tool}，请安装后重新检测。`);
  }
  if (!docker) {
    progress('检查管理机上的平台域名与泛解析。');
    if (!(await checkDns({ domain: installation.domain, entryIp: installation.entryIp, local: false }, { signal })).passed) {
      throw new ConfigError(`管理机 DNS 检查未通过。请在内网 DNS 配置 *.${installation.domain} A ${installation.entryIp}，让管理机和节点使用该 DNS 后重试。尚未创建集群。`);
    }
  }

  const cache = target.bundleDir || join(dirname(directory), 'resources');
  progress('按所选应用及依赖检查实际文件；HTTP 404 不会通过。');
  await prepareInstallFiles(plan, cache, target.offline, signal, progress);
  target.workDir = docker && context.installationKey ? localCluster(context.installationKey, installation.domain).workDir : join(directory, 'work');
  const bundle = join(target.workDir, 'bundle');
  await mkdir(bundle, { recursive: true, mode: 0o700 });
  // Stage only verified, selected files, never an entire arbitrary user directory.
  for (const file of plan.files) {
    signal.throwIfAborted();
    const destination = join(bundle, file.path);
    await mkdir(dirname(destination), { recursive: true, mode: 0o700 });
    await copyFile(join(cache, file.path), destination, constants.COPYFILE_FICLONE);
  }
  await prepareInstallFiles(plan, bundle, true, signal, () => {});
  await writeFile(join(bundle, 'SHA256SUMS'), plan.files.map(file => `${file.sha256}  ${file.path}`).join('\n') + '\n', { mode: 0o600 });
  if (plan.images.length) {
    await mkdir(join(bundle, 'images'), { recursive: true, mode: 0o700 });
    await writeFile(join(bundle, 'images/install-manifest.json'), JSON.stringify(plan.images.map(image => {
      const file = plan.files.find(file => file.path === image.file)!;
      return { ...image, size: file.size, sha256: file.sha256 };
    })), { mode: 0o600 });
  }

  const cluster = `apeiron-${hash(target.workDir)}`;
  const nodeName = docker ? `k3d-${cluster}-server-0` : nodes.find(node => node.role === 'server')!.name;
  const replacements: Record<string, string> = {
    __DOMAIN__: installation.domain, __REGISTRY__: `registry.${installation.domain}`, __NODE__: nodeName,
    __WORK__: docker ? '/work/state' : join(target.workDir, 'state'), __BUNDLE__: docker ? '/work/bundle' : bundle, __ARCH__: architecture,
  };
  let source = Bun.YAML.stringify(plan.environment);
  for (const [key, value] of Object.entries(replacements)) source = source.replaceAll(key, value);
  const values = Bun.YAML.parse(source) as Record<string, any>;
  Object.assign(values, { topology: installation.topology, domain: installation.domain, registry: `registry.${installation.domain}`, node: nodeName, work: replacements.__WORK__, bundle: replacements.__BUNDLE__, architecture });
  values.publicHttpsPort = installation.httpsPort;
  values.publicHttpPort = installation.httpPort;
  values.releases ??= {};
  if (!docker) {
    const storageNodes = [nodeName];
    const weed = values.releases.seaweedfs ??= {}; weed.values ??= {};
    Object.assign(weed.values, { nodes: storageNodes, replicas: storageNodes.length, replication: storageNodes.length === 3 ? '001' : '000',
      peers: storageNodes.map((_, index) => `seaweedfs-${index}.seaweedfs-peers.seaweedfs.svc.cluster.local:9333`).join(',') });
    const longhorn = values.releases.longhorn ??= {}; longhorn.values ??= {};
    longhorn.enabled = false;
    values.storageClass = 'local-path';
  }
  return { target: { ...target, environment: join(directory, 'input.yaml'), kubeconfig: join(target.workDir, 'state/kubeconfig') },
    source: Bun.YAML.stringify(values), runtime: { cluster, nodes, plan, bundle, architecture } };
}

type Runtime = { cluster: string; nodes: NodeTarget[]; plan: ReturnType<typeof installPlan>; bundle: string; architecture: 'amd64' | 'arm64' };

export async function finishClusterAccess(target: DeploymentTarget, directory: string, env: NodeJS.ProcessEnv, run: RunCommand) {
  const command = target.runner === 'docker' ? 'bash' : 'python3';
  const args = target.runner === 'docker'
    ? [join(target.root, 'tests/lab/helmfile.sh'), 'configure-oidc']
    : ['-m', 'chentu.cluster_oidc', '--inventory', join(directory, 'inventory.yaml')];
  const code = await run(command, args, target.root, env);
  if (code !== 0) throw new ConfigError(`集群 SSO 初始化或权限验收失败（退出码 ${code}）。请查看日志，修复后重新部署。`);
}

export async function bootstrapFresh(target: DeploymentTarget, environment: string, context: Context, runtime: Runtime) {
  const { directory, run, progress } = context;
  const { nodes, plan, bundle, cluster } = runtime;
  const installation = target.installation!;
  const env = { ...process.env, CHENTU_ROOT: target.root, CHENTU_ENV: environment,
    KUBECONFIG: target.kubeconfig, PYTHONPATH: join(target.root, 'cli/src'), UV_OFFLINE: '1', UV_PYTHON_DOWNLOADS: 'never',
    HELMFILE_NO_COLOR: 'true', HELMFILE_LOG_LEVEL: 'info' };
  async function checked(command: string, args: string[], message: string, processEnv = env) {
    context.signal.throwIfAborted(); progress(message);
    const code = await run(command, args, target.root, processEnv);
    if (code !== 0) throw new ConfigError(`${message}失败（退出码 ${code}），请查看本机安装日志。`);
  }
  if (target.runner === 'docker') {
    const container = cluster + '-setup-' + hash(directory);
    let image = plan.target.toolboxImage;
    if (plan.target.toolboxArchive) {
      image = plan.target.toolboxImageId!;
      await checked('docker', ['load', '-i', join(bundle, plan.target.toolboxArchive)], '导入已校验的 K3d 工具箱');
      await checked('docker', ['image', 'inspect', '--format', '{{.Id}}', image], '校验工具箱镜像 ID');
      for (const archive of plan.target.dockerArchives ?? []) {
        await checked('docker', ['load', '-i', join(bundle, archive.file)], '导入已校验的 K3d 系统镜像');
        await checked('docker', ['run', '--rm', '--name', container, '--network=none', '--entrypoint', '/opt/chentu-venv/bin/python',
          '-v', '/var/run/docker.sock:/var/run/docker.sock', image, '-c',
          'import json,subprocess,sys; images=json.loads(sys.argv[1]); assert all(subprocess.check_output(["docker","image","inspect","--format","{{.Id}}",x["name"]],text=True).strip()==x["id"] for x in images), "K3d system image ID mismatch"',
          JSON.stringify(archive.images)], '校验 K3d 系统镜像 ID');
      }
    } else {
      if (!image || !/^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*@sha256:[a-f0-9]{64}$/.test(image)) throw new ConfigError('安装包未声明固定 digest 的 K3d 工具箱镜像。');
      await checked('docker', ['pull', image], '下载并校验 K3d 工具箱镜像');
    }
    target.image = image;
    const marker = join(target.workDir, 'installation.json');
    const checkedCluster = await checkLocalCluster({ workDir: target.workDir, cluster }, installation, context.signal);
    await writeFile(marker, JSON.stringify({ cluster, domain: installation.domain }), { mode: 0o600 });
    progress(checkedCluster.message);
    const labEnv = { ...env, LAB_ENV: environment, LAB_PLATFORM: `linux/${runtime.architecture}`, LAB_EXPECT_ARCH: runtime.architecture, LAB_WORK_DIR: target.workDir, LAB_IMAGE: image, LAB_CLUSTER: cluster, LAB_CONTAINER: container, LAB_REGISTRY: `k3d-${cluster}-reg:5000`, LAB_HTTP_PORT: String(installation.httpPort), LAB_HTTPS_PORT: String(installation.httpsPort) };
    // Validate values before prepare creates the cluster. This invocation has no
    // Docker socket; it cannot create containers or mutate a Kubernetes cluster.
    await checked('docker', ['run', '--rm', '--name', container, '--network=none', '--entrypoint', '/opt/chentu-venv/bin/python',
      '-v', `${target.root}:/repo:ro`, '-v', `${environment}:/environment.yaml:ro`, '-v', `${target.workDir}:/work`,
      '-e', 'CHENTU_ENV=/environment.yaml', '-e', 'PYTHONPATH=/repo/cli/src',
      image, '/repo/deploy/helmfile/scripts/check.py'], '校验应用配置');
    await checked('bash', [join(target.root, 'tests/lab/helmfile.sh'), 'prepare'], '创建本地 K3d 测试集群', labEnv);
    return labEnv;
  }
  await checked('python3', [join(target.root, 'deploy/helmfile/scripts/check.py')], '校验应用配置');
  const remoteBundle = installation.topology === 'multi-k3s' ? `/opt/chentu/bundles/${hash(directory)}` : bundle;
  const inventory = join(directory, 'inventory.yaml');
  await writeFile(inventory, Bun.YAML.stringify(inventoryFor(installation, nodes, remoteBundle, target.kubeconfig, runtime.architecture)), { mode: 0o600 });
  const copyTasks = installation.topology === 'multi-k3s' ? [
    { name: 'Create resource directories', 'ansible.builtin.file': { path: '{{ chentu_bundle }}/{{ item }}', state: 'directory', mode: '0700' }, loop: [...new Set(plan.files.map(file => dirname(file.path)))] },
    { name: 'Copy verified resources', 'ansible.builtin.copy': { src: `${bundle}/{{ item }}`, dest: '{{ chentu_bundle }}/{{ item }}', mode: '0600' }, loop: [...plan.files.map(file => file.path), 'SHA256SUMS'] },
  ] : [];
  const preflight = join(directory, 'prepare-hosts.yaml');
  const tasks = [
    { name: 'Verify platform DNS on each node', 'ansible.builtin.command': { argv: ['python3', '-c',
      'import socket,sys; domain,ip=sys.argv[1:]; hosts=["apeiron."+domain,"iam."+domain]; assert all({a[4][0] for a in socket.getaddrinfo(h,443,type=socket.SOCK_STREAM)}=={ip} for h in hosts), "Platform DNS does not match the entry IP"',
      installation.domain, installation.entryIp] }, changed_when: false, async: 15, poll: 1 },
    // Check SSH transport between peers before bootstrapping. K3s service ports
    // aren't listening yet; their readiness is checked by Chentu during joins.
    ...(installation.topology === 'multi-k3s' ? [{ name: 'Check peer SSH reachability', 'ansible.builtin.wait_for': { host: '{{ item }}', port: installation.sshPort, timeout: 8, connect_timeout: 3 }, loop: nodes.map(node => node.address) }] : []),
    ...copyTasks,
    { name: 'Verify staged bundle on each node', 'ansible.builtin.command': { argv: ['sha256sum', '--check', 'SHA256SUMS'], chdir: '{{ chentu_bundle }}' }, changed_when: false },
  ];
  await writeFile(preflight, Bun.YAML.stringify([{ name: 'Prepare verified installation resources', hosts: 'server:agent', become: true, gather_facts: false, tasks }]), { mode: 0o600 });
  await checked('ansible-playbook', ['-i', inventory, preflight], '检查节点互联并准备安装包');
  await checked('ansible-playbook', ['-i', inventory, join(target.root, 'bootstrap/hosts.yaml')], '安装 K3s 并生成 kubeconfig');
  return env;
}
