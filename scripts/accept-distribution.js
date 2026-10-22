import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { acceptanceTargets, validateDistribution, validateAcceptance } from './lib/distribution.js';

// Destructive installation is restricted to an explicitly configured disposable
// acceptance runner. Never use a developer's existing cluster as this runner.
const [targetName, out, inputPath, work] = process.argv.slice(2);
const target = acceptanceTargets[targetName];
if (!target || ![out, inputPath, work, process.env.APEIRON_ACCEPTANCE_CONFIG].every(p => p && isAbsolute(p))
    || process.env.APEIRON_RELEASE_DISPOSABLE !== 'true')
  throw new Error('Requires target, absolute release/input/new-work paths and a configured disposable runner');
if (process.platform !== target.os || process.arch !== target.arch) throw new Error('Wrong acceptance host architecture');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const json = async path => JSON.parse(await readFile(path, 'utf8'));
const release = await json(join(out, 'release.json'));
const inputBytes = await readFile(inputPath);
const distribution = validateDistribution(JSON.parse(inputBytes), release);
const profile = await json(process.env.APEIRON_ACCEPTANCE_CONFIG);
const installation = profile.deployment?.installation;
if (installation?.topology !== target.topology || !profile.apps?.includes('apeiron')
    || profile.deployment.offline !== false || profile.deployment.root || profile.deployment.environment)
  throw new Error('Acceptance must install Apeiron from the compiled public pins on the selected topology');
if (target.servers === 3 && (installation.ha !== true || installation.nodes?.length !== 3
    || installation.nodes.some(n => n.role !== 'server')))
  throw new Error('Multi-node acceptance requires three distinct server nodes');
let osRelease = null;
if (target.topology !== 'single-k3d') {
  const os = await readFile('/etc/os-release', 'utf8');
  if (!/^ID=ubuntu$/m.test(os) || !/^VERSION_ID="22\.04"$/m.test(os)) throw new Error('Requires Ubuntu 22.04');
  osRelease = 'ubuntu:22.04';
}
const archive = join(out, `apeiron-${release.version}-${target.cli}.tar.gz`);
const archiveSha256 = hash(await readFile(archive));
if (archiveSha256 !== release.files?.[`apeiron-${release.version}-${target.cli}.tar.gz`]?.sha256)
  throw new Error('Candidate archive checksum mismatch');
await mkdir(work, { mode: 0o700 }); // Exclusive: never resume an unrelated installation.
async function run(args, stdin, env) {
  const child = Bun.spawn(args, { stdin: stdin === undefined ? 'ignore' : new Blob([stdin]), stdout: 'pipe', stderr: 'pipe', env });
  const [stdout, , code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  // Tool output can contain installation credentials; errors deliberately omit it.
  if (code !== 0) throw new Error(`${args[0]} failed during acceptance (exit ${code})`);
  return stdout;
}
const members = (await run(['tar', '-tzf', archive])).trim().split('\n').sort();
if (JSON.stringify(members) !== JSON.stringify(['README.md', 'apeiron', 'build-provenance.json'].sort()))
  throw new Error('Unexpected release archive members');
if ((await run(['tar', '-tvzf', archive])).trim().split('\n').some(line => !line.startsWith('-')))
  throw new Error('Archive contains non-regular files');
await run(['tar', '-xzf', archive, '-C', work]);
const binary = join(work, 'apeiron');
const info = JSON.parse(await run([binary, '--build-info']));
if (info.version !== release.version || info.revision !== release.revision || info.clean !== true)
  throw new Error('Candidate build identity mismatch');
const configPath = join(work, 'config.json');
const env = { ...process.env };
for (const key of Object.keys(env)) if (/^(APEIRON_TEST|APEIRON_FIXTURE|CHENTU_|LAB_|KUBECONFIG$)/.test(key)) delete env[key];
const child = spawn(binary, ['init', '--no-open', '--config', configPath], { env, stdio: ['ignore', 'pipe', 'ignore'] });
const exited = new Promise(resolve => child.once('close', resolve));
try {
  const url = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('Wizard startup timed out')), 30_000);
    let output = '';
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.on('close', () => { clearTimeout(timer); reject(new Error('Wizard exited')); });
    child.stdout.on('data', bytes => {
      output = (output + bytes).slice(-16384);
      const match = /url\t(http:\/\/127\.0\.0\.1:\d+\/setup\/[a-f0-9]+\/)/.exec(output);
      if (match) { clearTimeout(timer); resolve(match[1]); }
    });
  });
  async function api(path, body) {
    const response = await fetch(url + 'api/' + path, { method: body === undefined ? 'GET' : 'POST',
      headers: { origin: new URL(url).origin, 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(180_000) });
    if (!response.ok) throw new Error(`Wizard ${path}: HTTP ${response.status}; inspect private installation log`);
    return response.json();
  }
  const current = await api('config');
  if (current.revision !== null || current.deployment.phase !== 'idle') throw new Error('Acceptance config is not fresh');
  await api('deploy', { ...profile, revision: null });
  const deadline = Date.now() + 90 * 60_000;
  let deployment;
  for (;;) {
    deployment = await api('deployment');
    if (deployment.phase === 'succeeded') break;
    if (!['preparing', 'running'].includes(deployment.phase) || Date.now() > deadline)
      throw new Error('Installation did not succeed; inspect private installation log');
    await new Promise(resolve => setTimeout(resolve, 5000));
  }
  const record = await json(configPath + '.installation-complete.json');
  if (record.succeeded !== true || record.cliSha256 !== hash(await readFile(binary)))
    throw new Error('Completed installation is not bound to the candidate binary');
  const t = record.target;
  const command = t.runner === 'native'
    ? [t.kubectlPath, '--kubeconfig', t.kubeconfig]
    : ['docker', 'run', '--rm', '--pull=never', '--network', record.environment.LAB_NETWORK || `k3d-${record.environment.LAB_CLUSTER}`,
      '--entrypoint', 'kubectl', '-v', `${t.workDir}/state/kubeconfig:/kubeconfig:ro`, t.image, '--kubeconfig', '/kubeconfig'];
  const nodes = JSON.parse(await run([...command, 'get', 'nodes', '-o', 'json'])).items;
  const servers = nodes.filter(n => Object.hasOwn(n.metadata.labels, 'node-role.kubernetes.io/control-plane')).length;
  if (servers !== target.servers || nodes.length !== servers || nodes.some(n => !n.status.conditions.some(c => c.type === 'Ready' && c.status === 'True')))
    throw new Error('Cluster node roles/count/readiness mismatch');
  const architecture = target.arch === 'x64' ? 'amd64' : 'arm64';
  if (nodes.some(n => n.status.nodeInfo?.architecture !== architecture
      || (target.topology !== 'single-k3d' && !n.status.nodeInfo?.osImage?.startsWith('Ubuntu 22.04'))))
    throw new Error('Cluster node architecture/OS mismatch');
  const pods = JSON.parse(await run([...command, 'get', 'pods', '-n', 'apeiron', '-o', 'json'])).items;
  for (const component of ['backend', 'app', 'ops', 'scode']) {
    const expected = distribution.apeiron.images[component][architecture].split('@')[1];
    if (!pods.some(p => p.status.containerStatuses?.some(c => c.ready === true && c.imageID?.endsWith('@' + expected))))
      throw new Error(`Running ${component} image does not match the distribution digest`);
  }
  const credentials = await api('credentials', {});
  if (!credentials.username || !credentials.password) throw new Error('Durable operator credentials missing');
  const access = deployment.access;
  const ca = access?.ca?.path;
  if (!ca || access.domain !== installation.domain || access.entryIp !== installation.entryIp)
    throw new Error('Installation access identity mismatch');
  const port = installation.httpsPort;
  const origin = host => `https://${host}${port === 443 ? '' : ':' + port}`;
  const curl = host => ['curl', '--disable', '--silent', '--show-error', '--fail', '--noproxy', '*', '--max-time', '30',
    '--proto', '=https', '--cacert', ca, '--resolve', `${host}:${port}:${installation.entryIp}`];
  for (const name of ['apeiron', 'ops', 'iam']) {
    const host = `${name}.${installation.domain}`;
    await run([...curl(host), '--head', origin(host) + '/']);
  }
  // A real Keycloak token request verifies that the durable operator can log in;
  // a successful credential read alone is insufficient. Never persist the token.
  const host = `iam.${installation.domain}`;
  const form = new URLSearchParams({ grant_type: 'password', client_id: 'admin-cli', username: credentials.username, password: credentials.password });
  const token = JSON.parse(await run([...curl(host), '-H', 'Content-Type: application/x-www-form-urlencoded', '--data-binary', '@-',
    origin(host) + '/realms/master/protocol/openid-connect/token'], form.toString()));
  if (!token.access_token || token.token_type?.toLowerCase() !== 'bearer') throw new Error('Operator login did not issue a token');
  const receipt = { schemaVersion: 1, version: release.version, revision: release.revision, target: targetName,
    runId: process.env.GITHUB_RUN_ID, distributionSha256: hash(inputBytes), archiveSha256,
    os: process.platform, arch: process.arch, osRelease, topology: target.topology, servers, agents: nodes.length - servers,
    checks: { installation: true, nodes: true, images: true, credentials: true, https: true, login: true } };
  validateAcceptance(receipt, release, hash(inputBytes), process.env.GITHUB_RUN_ID);
  await writeFile(join(work, `${targetName}.json`), JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  console.log(`PASS ${targetName}: installation, nodes, durable credentials, HTTPS and IAM login`);
} finally {
  child.kill('SIGTERM');
  const timeout = setTimeout(() => child.kill('SIGKILL'), 10000);
  await exited;
  clearTimeout(timeout);
}
