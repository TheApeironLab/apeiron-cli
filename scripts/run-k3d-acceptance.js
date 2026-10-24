import { createHash } from 'node:crypto';
import { createServer } from 'node:net';
import { lstat, readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { acceptanceTargets } from './lib/distribution.js';
import { cleanupK3dLease } from './lib/k3d-lease.js';
import { acceptanceScope } from './lib/acceptance-scope.js';
const [targetName, release, input, work] = process.argv.slice(2);
const target = acceptanceTargets[targetName];
if (!target || target.topology !== 'single-k3d' || ![release, input, work].every(isAbsolute)
    || process.env.APEIRON_RELEASE_DISPOSABLE !== 'true') throw new Error('Requires a configured disposable ARM64 K3d runner');
if (await lstat(work).catch(error => { if (error.code === 'ENOENT') return null; throw error; }))
  throw new Error('Acceptance directory already exists');
const id = `${process.env.GITHUB_RUN_ID}-${process.env.GITHUB_RUN_ATTEMPT}`;
if (!/^[1-9][0-9]*-[1-9][0-9]*$/.test(id)) throw new Error('Requires Actions run identity');
async function freePort() {
  const server = createServer();
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}
const httpPort = await freePort();
let httpsPort = await freePort();
while (httpsPort === httpPort) httpsPort = await freePort();
const domain = `release-${id}-${target.os}.apeironlab.internal`;
const profile = { schemaVersion: 1, slug: `release-${process.env.GITHUB_RUN_ID}`,
  apps: acceptanceScope(JSON.parse(await readFile(join(release, 'release.json'))).version,
    JSON.parse(await readFile(input)).applications).applications,
  models: { provider: 'fixture', baseUrl: 'https://models.example.internal/v1', apiKey: 'fixture-only-key', fast: 'fixture-chat', deep: 'fixture-chat' },
  deployment: { offline: false, installation: { topology: 'single-k3d', ha: false, nodes: [],
    domain, entryIp: '127.0.0.1', httpPort, httpsPort, sshKey: '', sshUser: '', sshPort: 22 } } };
await writeFile(work + '.profile.json', JSON.stringify(profile), { flag: 'wx', mode: 0o600 });
const hash = text => createHash('sha256').update(text).digest('hex').slice(0, 12);
const directory = join(work, 'deployments', 'k3d-' + hash(join(work, 'config.json') + '|' + domain));
const cluster = 'apeiron-' + hash(directory);
async function docker(args) {
  const child = Bun.spawn(['docker', ...args], { stdout: 'pipe', stderr: 'pipe' });
  const [text, , code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (code) throw new Error('Cannot clean up this acceptance cluster; inspect the private runner');
  return text.trim().split('\n').filter(Boolean);
}
try {
  const child = Bun.spawn([process.execPath, new URL('./accept-distribution.js', import.meta.url).pathname,
    targetName, release, input, work], { stdout: 'inherit', stderr: 'inherit',
    env: { ...process.env, APEIRON_ACCEPTANCE_CONFIG: work + '.profile.json' } });
  if (await child.exited) throw new Error('K3d public-download acceptance failed');
} finally {
  // The CLI derives this name from the exclusive config path and unique domain.
  // Never prune Docker or remove resources belonging to another cluster.
  await cleanupK3dLease(cluster, docker);
}
