import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { DeploymentTarget } from '../src/init/config';

export const requiredApps = ['vasi', 'apeiron', 'ontology', 'task', 'corpus', 'matrix', 'nexus', 'stalwart'];
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
  const source = `domain: example.internal\nregistry: registry.example.internal\nwork: /work/state\nbundle: /work/bundle\nfixtureCalls: ${JSON.stringify(calls)}\nfixtureExit: 0\nfixtureDelay: 700\nreleases:\n  apeiron:\n    values:\n      image: test-only-image\n      models: {keep: true}\n  postgres:\n    enabled: true\n    values: {storage: 10Gi}\n`;
  await writeFile(environment, source);
  const stub = join(dir, 'fixture.ts');
  await writeFile(stub, `import { appendFile } from 'node:fs/promises';
const env = process.env[process.env.FIXTURE_ENV!];
const values = Bun.YAML.parse(await Bun.file(env!).text()) as any;
await appendFile(values.fixtureCalls, JSON.stringify({args: process.argv.slice(2), environment: env, values, profile: process.env.CHENTU_PROFILE, kubeconfig: process.env.KUBECONFIG, workDir: process.env.LAB_WORK_DIR})+'\\n');
console.log('Upgrading release=apeiron, chart=fixture');
console.log('private-log-test-key');
await Bun.sleep(values.fixtureDelay);
process.exit(values.fixtureExit);
`);
  const quote = (value: string) => "'" + value.replaceAll("'", "'\"'\"'") + "'";
  const script = `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(stub)} "$@"\n`;
  await writeFile(join(root, 'tests/lab/helmfile.sh'), script.replace('exec ', 'export FIXTURE_ENV=LAB_ENV\nexec '));
  await writeFile(join(root, 'deploy/helmfile/run.sh'), script.replace('exec ', 'export FIXTURE_ENV=CHENTU_ENV\nexec '));
  for (const name of ['docker', 'helmfile', 'helm']) await writeFile(join(bin, name), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  const target: DeploymentTarget = { runner: 'docker', root, environment, workDir, image: 'fixture-toolbox', profile: 'local', kubeconfig: '' };
  return { root, workDir, environment, calls, bin, source, target };
}
