import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { DeploymentTarget } from '../src/init/config';
import { APPS } from '../src/init/config';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

export const requiredApps = ['nexus', 'vasi', 'ontology', 'apeiron'];
export async function deploymentFixture(dir: string) {
  const root = join(dir, 'chentu fixture');
  const workDir = join(dir, 'work');
  const environment = join(dir, 'base.yaml');
  const calls = join(dir, 'calls.jsonl');
  const bin = join(dir, 'bin');
  await Promise.all([
    mkdir(join(root, 'tests/lab'), { recursive: true }), mkdir(join(root, 'deploy/helmfile'), { recursive: true }),
    mkdir(join(workDir, 'state'), { recursive: true }), mkdir(bin),
  ]);
  await writeFile(join(workDir, 'state/kubeconfig'), 'test-only-kubeconfig');
  const source = `topology: single-k3d\narchitecture: arm64\ndomain: example.internal\nregistry: registry.example.internal\nwork: /work/state\nbundle: /work/bundle\nfixtureCalls: ${JSON.stringify(calls)}\nfixtureExit: 0\nfixtureDelay: 700\nreleases:\n  apeiron:\n    values:\n      image: test-only-image\n      models: {keep: true}\n  postgres:\n    enabled: true\n    values: {storage: 10Gi}\n`;
  await writeFile(environment, source);
  const stub = join(dir, 'fixture.ts');
  await writeFile(stub, `import { appendFile, mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
const env = process.env[process.env.FIXTURE_ENV!];
const values = Bun.YAML.parse(await Bun.file(env!).text()) as any;
await appendFile(values.fixtureCalls, JSON.stringify({args: process.argv.slice(2), environment: env, values, legacyProfile: process.env.CHENTU_PROFILE, kubeconfig: process.env.KUBECONFIG, workDir: process.env.LAB_WORK_DIR, httpsPort: process.env.LAB_HTTPS_PORT})+'\\n');
if (process.argv[2] === 'prepare') {
  await mkdir(join(process.env.LAB_WORK_DIR!, 'state'), { recursive: true });
  await writeFile(join(process.env.LAB_WORK_DIR!, 'state/kubeconfig'), 'fixture-kubeconfig');
  process.exit(0);
}
if (process.argv[2] === 'configure-oidc') {
  await Bun.sleep(values.fixtureOidcDelay ?? 0);
  process.exit(values.fixtureOidcExit ?? 0);
}
console.log('Upgrading release=apeiron, chart=fixture');
console.log('private-log-test-key');
if (values.fixtureExit === 0 && values.fixtureCa) await writeFile(join(process.env.LAB_WORK_DIR!, 'state/chentu-ca.crt'), await Bun.file(values.fixtureCa).text());
await Bun.sleep(values.fixtureDelay);
process.exit(values.fixtureExit);
`);
  const quote = (value: string) => "'" + value.replaceAll("'", "'\"'\"'") + "'";
  const script = `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(stub)} "$@"\n`;
  await writeFile(join(root, 'tests/lab/helmfile.sh'), script.replace('exec ', 'export FIXTURE_ENV=LAB_ENV\nexec '));
  await writeFile(join(root, 'deploy/helmfile/run.sh'), script.replace('exec ', 'export FIXTURE_ENV=CHENTU_ENV\nexec '));
  for (const name of ['docker', 'helmfile', 'helm']) await writeFile(join(bin, name), `#!/bin/sh\nif [ "$1" = inspect ]; then echo "Error: No such object: fixture" >&2; exit 1; fi\ncase "$*" in *"list --pending"*) echo '[]';; esac\nexit 0\n`, { mode: 0o755 });
  const target: DeploymentTarget = { runner: 'docker', root, environment, workDir, image: 'fixture-toolbox', kubeconfig: '', offline: false, bundleDir: '' };
  return { root, workDir, environment, calls, bin, source, target };
}

export async function freshInstallFixture(dir: string, setup: Awaited<ReturnType<typeof deploymentFixture>>, exit = 0, resumeDomain?: string) {
  if (resumeDomain) {
    // UI tests run against an owned mock cluster, independently of services on
    // the developer's public ports. Actual first-install port checks stay enabled.
    const hash = (text: string) => createHash('sha256').update(text).digest('hex').slice(0, 12);
    const work = join(dir, 'deployments', 'k3d-' + hash(join(dir, 'config.json') + '|' + resumeDomain));
    const cluster = 'apeiron-' + hash(work);
    await mkdir(work, { recursive: true });
    await writeFile(join(work, 'installation.json'), JSON.stringify({ cluster, domain: resumeDomain }));
    const secret = JSON.stringify({ metadata: { name: 'keycloak-bootstrap', namespace: 'keycloak' }, data: {
      username: Buffer.from('fixture-admin').toString('base64'), password: Buffer.from('fixture-only-admin-password').toString('base64'),
    } });
    await writeFile(join(setup.bin, 'docker'), `#!/bin/sh\nif [ "$1" = inspect ]; then echo '{"cluster":"${cluster}","running":true,"bindings":{"54320/tcp":[{"HostIp":"127.0.0.1","HostPort":"54320"}],"443/tcp":[{"HostIp":"127.0.0.1","HostPort":"54321"}]}}'; exit 0; fi\nif [ "$1" = inspect ]; then printf '%s\\n' '${cluster}'; fi\ncase "$*" in *' list --pending '*) echo '[]';; *' get secret keycloak-bootstrap '*) printf '%s\\n' '${secret}';; esac\nexit 0\n`, { mode: 0o755 });
  }
  const bytes = 'fixture-only resource';
  const ca = join(dir, 'fixture-ca.crt');
  if (!await Bun.file(ca).exists()) await promisify(execFile)('openssl', ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '1',
    '-subj', '/CN=Apeiron test fixture only', '-addext', 'basicConstraints=critical,CA:TRUE', '-keyout', join(dir, 'fixture-ca.key'), '-out', ca]);
  await mkdir(join(setup.root, 'cli/src/chentu/lab'), { recursive: true });
  await mkdir(join(setup.root, 'bootstrap'), { recursive: true });
  await writeFile(join(setup.root, 'bootstrap/oidc.yaml'), 'fixture-only');
  await writeFile(join(setup.root, 'cli/src/chentu/cluster_oidc.py'), '# fixture-only');
  await writeFile(join(setup.root, 'cli/src/chentu/__init__.py'), '');
  await writeFile(join(setup.root, 'cli/src/chentu/lab/__init__.py'), '');
  await writeFile(join(setup.root, 'cli/src/chentu/lab/ingresshosts.py'), 'import sys\nfor name in ["apeiron", "iam", "task"]: print(name+"."+sys.argv[2]+":"+sys.argv[3])\n');
  await mkdir(join(dir, 'deployments/resources'), { recursive: true });
  await writeFile(join(dir, 'deployments/resources/core.bin'), bytes);
  await mkdir(join(setup.root, 'setup'), { recursive: true });
  const environment = Bun.YAML.parse(setup.source.replace('fixtureExit: 0', `fixtureExit: ${exit}`).replace('fixtureDelay: 700', 'fixtureDelay: 1500'));
  (environment as Record<string, unknown>).fixtureCa = ca;
  const profile = { deploymentTopology: true, publicPorts: true, clusterOidc: true, base: ['core', 'cluster-access'], environment, toolboxImage: 'fixture/toolbox@sha256:' + 'a'.repeat(64) };
  delete (environment as Record<string, unknown>).architecture;
  const catalog = { schemaVersion: 1, targets: { 'k3d-arm64': profile, 'k3d-amd64': profile },
    components: { core: { requires: [], files: ['core.bin'] }, 'cluster-access': { requires: ['core'], files: [] }, ...Object.fromEntries(APPS.map(app => [app.id, { requires: ['core'], files: [] }])) },
    files: [{ path: 'core.bin', size: Buffer.byteLength(bytes), sha256: createHash('sha256').update(bytes).digest('hex') }] };
  await writeFile(join(setup.root, 'setup/install.json'), JSON.stringify(catalog));
}
