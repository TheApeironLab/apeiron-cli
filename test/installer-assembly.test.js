import { afterEach, expect, test } from 'bun:test';
import { mkdtemp, writeFile, mkdir, symlink, rm, chmod, lstat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { digest, inventory, verifyChecksums, checkProfile, safePath, startScript, planAssembly, requireAssemblyProof, assemble, hashFile } from '../scripts/lib/installer-assembly.js';
const roots = [];
afterEach(async () => { for (const p of roots.splice(0)) await rm(p, { recursive: true, force: true }); });
async function fixture() { const p = await mkdtemp(join(tmpdir(), 'installer-')); roots.push(p); return p; }
function profile(contract = 'legacy-native') { return { contract, config: { apps: ['apeiron'], deployment: { offline: true, runner: 'native', installation: { topology: 'multi-k3s', nodes: [1, 2, 3].map(i => ({ host: `192.0.2.${i}`, address: `192.0.2.${i}`, role: i === 1 || contract === 'hosted-first' ? 'server' : 'agent' })) } } } }; }
test('contracts fail closed for unknown roles, live addresses and online configuration', () => {
  expect(checkProfile(profile()).contract).toBe('legacy-native');
  expect(checkProfile(profile('hosted-first')).contract).toBe('hosted-first');
  expect(() => checkProfile(profile('unknown'))).toThrow('Unknown');
  const p = profile(); p.config.deployment.installation.nodes[1].role = 'server';
  expect(() => checkProfile(p)).toThrow('roles');
  const q = profile(); q.config.deployment.offline = false;
  expect(() => checkProfile(q)).toThrow('offline');
  const r = profile(); r.config.deployment.installation.sshKey = '/private/key';
  expect(() => checkProfile(r)).toThrow('credentials');
});
test('hosted-first profiles accept the three publisher topologies with their exact default shapes', () => {
  const base = { contract: 'hosted-first', config: { schemaVersion: 2, slug: 'example-team', apps: ['nexus', 'vasi', 'ontology', 'apeiron', 'browser', 'ci'], models: { provider: 'external', baseUrl: 'https://models.example.internal/v1', apiKey: '' } } };
  const single = (topology, runner, extra) => ({ ...base, config: { ...base.config, deployment: { runner, root: '', offline: true, bundleDir: '', installation: { topology, ha: false, domain: 'example.internal', sshKey: '', sshUser: '', sshPort: 22, nodes: [], ...extra } } } });
  const k3d = single('single-k3d', 'docker', { entryIp: '127.0.0.1', httpPort: 54320, httpsPort: 54321 });
  const k3s = single('single-k3s', 'native', { entryIp: '', httpPort: 80, httpsPort: 443 });
  expect(checkProfile(k3d).contract).toBe('hosted-first');
  expect(checkProfile(k3s).contract).toBe('hosted-first');
  expect(checkProfile(profile('hosted-first')).contract).toBe('hosted-first');
  // Legacy packages stay multi-k3s only.
  expect(() => checkProfile({ ...k3s, contract: 'legacy-native' })).toThrow('multi-k3s');
  // Runner, nodes, ports and addresses are pinned per topology.
  expect(() => checkProfile(single('single-k3d', 'native', { entryIp: '127.0.0.1', httpPort: 54320, httpsPort: 54321 }))).toThrow('Runner');
  expect(() => checkProfile(single('single-k3d', 'docker', { entryIp: '127.0.0.1', httpPort: 80, httpsPort: 443 }))).toThrow('54320');
  expect(() => checkProfile(single('single-k3s', 'native', { entryIp: '10.0.0.5', httpPort: 80, httpsPort: 443 }))).toThrow('live addresses');
  const withNode = single('single-k3s', 'native', { entryIp: '', httpPort: 80, httpsPort: 443 });
  withNode.config.deployment.installation.nodes = [{ host: '192.0.2.1', address: '192.0.2.1', role: 'server' }];
  expect(() => checkProfile(withNode)).toThrow('no nodes');
  const hostedMulti = profile('hosted-first'); hostedMulti.config.deployment.installation.nodes.pop();
  expect(() => checkProfile(hostedMulti)).toThrow('roles');
});
test('default profiles reject model API credentials before they can enter provenance', () => {
  for (const key of ['models', 'llm']) {
    const value = profile(); value.config[key] = { apiKey: 'sk-private-profile-key' };
    expect(() => checkProfile(value)).toThrow('API credentials');
    value.config[key].apiKey = '';
    expect(() => checkProfile(value)).not.toThrow();
  }
});
test('inventory rejects traversal, symlinks and unlisted resources', async () => {
  for (const p of ['/abs', '../evil', 'x/../evil', 'x\\evil', 'a\nb']) expect(() => safePath(p)).toThrow();
  const root = await fixture();
  await writeFile(join(root, 'resource'), 'data');
  await writeFile(join(root, 'SHA256SUMS'), `${digest('data')}  resource\n`);
  await verifyChecksums(root, await inventory(root));
  await writeFile(join(root, 'unexpected'), 'extra');
  await expect(verifyChecksums(root, await inventory(root))).rejects.toThrow('Unlisted');
  await symlink('resource', join(root, 'link'));
  await expect(inventory(root)).rejects.toThrow('regular');
});
test('checksums detect missing, changed and duplicate resources', async () => {
  const root = await fixture();
  await writeFile(join(root, 'resource'), 'data');
  const checksum = `${digest('data')}  resource\n`;
  await writeFile(join(root, 'SHA256SUMS'), checksum + checksum);
  await expect(verifyChecksums(root, await inventory(root))).rejects.toThrow('Duplicate');
  await writeFile(join(root, 'SHA256SUMS'), checksum);
  await writeFile(join(root, 'resource'), 'tamper');
  await expect(verifyChecksums(root, await inventory(root))).rejects.toThrow('changed');
  await rm(join(root, 'resource'));
  await expect(verifyChecksums(root, await inventory(root))).rejects.toThrow('Missing');
});
test('startup uses relative resources, isolated home state and no deployment invocation', async () => {
  const root = await fixture();
  const script = startScript('test-candidate', 'python/runtime.tar.gz');
  await writeFile(join(root, 'start.sh'), script);
  expect(Bun.spawnSync(['bash', '-n', join(root, 'start.sh')]).exitCode).toBe(0);
  expect(script).toContain('$HOME/.apeiron/installers/test-candidate');
  expect(script).toContain('"$HERE/bin/apeiron" init');
  expect(script).not.toContain(' deploy');
});
test('archive verification rejects changed and unlisted archived contents', async () => {
  const root = await fixture(); const candidate = join(root, 'candidate'); await mkdir(candidate);
  await writeFile(join(candidate, 'resource'), 'data');
  await writeFile(join(candidate, 'SHA256SUMS'), `${digest('data')}  resource\n`);
  const archive = join(root, 'candidate.tar');
  const tar = () => Bun.spawnSync(['tar', '-cf', archive, '-C', root, 'candidate'], { env: { ...process.env, COPYFILE_DISABLE: '1' } });
  const verify = () => Bun.spawnSync(['python3', 'scripts/lib/verify-installer-archive.py', archive, 'candidate'], { stdout: 'pipe', stderr: 'pipe' }).exitCode;
  tar(); expect(verify()).toBe(0);
  await writeFile(join(candidate, 'resource'), 'tampered'); tar(); expect(verify()).not.toBe(0);
});

async function planFixture() {
  const root = await fixture(); const bundle = join(root, 'bundle'); const cli = join(root, 'cli');
  await mkdir(join(bundle, 'python'), { recursive: true }); await mkdir(cli);
  await writeFile(join(bundle, 'python/runtime.tar.gz'), 'fixture runtime');
  await writeFile(join(bundle, 'SHA256SUMS'), `${digest('fixture runtime')}  python/runtime.tar.gz\n`);
  const bytes = Buffer.alloc(20); bytes.write('7f454c46', 0, 'hex'); bytes[4] = 2; bytes[5] = 1; bytes.writeUInt16LE(62, 18);
  await writeFile(join(cli, 'apeiron'), bytes);
  const source = { revision: 'a'.repeat(40), tree: 'b'.repeat(40), channel: '1.98.0' };
  const provenance = { schemaVersion: 1, kind: 'apeiron-cli-build', version: '0.2.1', toolchain: { channel: '1.98.0', rustc: 'rustc 1.98.0 (fixture)', cargo: 'cargo 1.98.0 (fixture)' }, target: 'linux-x64', triple: 'x86_64-unknown-linux-musl', source, binary: { size: bytes.length, sha256: digest(bytes) } };
  await writeFile(join(cli, 'build-provenance.json'), JSON.stringify(provenance));
  const p = profile(); p.expectedCli = { commit: source.revision }; p.bundleSource = { commit: 'c'.repeat(40) };
  const profilePath = join(root, 'profile.json'); await writeFile(profilePath, JSON.stringify(p));
  const validator = join(root, 'validator');
  const contract = { contract: 'legacy-native', target: { arch: 'amd64', ubuntu: 'jammy' }, validator: { sourceRevision: source.revision }, files: [{ path: 'python/runtime.tar.gz', size: 15, sha256: digest('fixture runtime') }], runtimeTools: ['python'], checks: { runtimeTools: 'passed' } };
  await writeFile(validator, '#!/usr/bin/env bun\nconsole.log(' + JSON.stringify(JSON.stringify(contract)) + ');\n'); await chmod(validator, 0o755);
  return { options: { bundle, cli, candidate: 'fixture', output: join(root, 'new-output'), profile: profilePath, validator }, provenance, profile: p };
}
test('plan validates identities and refuses output conflicts without writing output', async () => {
  const f = await planFixture(); const plan = await planAssembly(f.options);
  expect(plan.contract.contract).toBe('legacy-native');
  expect(plan.bindings.startScriptSha256).toHaveLength(64);
  expect(plan.toolSources.validator.sha256).toBe(await hashFile(f.options.validator));
  expect(plan.toolSources.assembly.kind).toBe('working-tree-tool-source');
  expect(plan.toolSources.assembly.files.map(f => f.path)).toContain('scripts/lib/verify-installer-archive.py');
  for (const file of plan.toolSources.assembly.files) expect(file.sha256).toBe(await hashFile(file.path));
  await mkdir(f.options.output);
  await expect(planAssembly(f.options)).rejects.toThrow('already exists');
  await rm(f.options.output, { recursive: true });
  f.profile.expectedCli.commit = 'd'.repeat(40);
  await writeFile(f.options.profile, JSON.stringify(f.profile));
  await expect(planAssembly(f.options)).rejects.toThrow('declared source identity');
});
test('plan rejects build provenance that does not bind actual binary bytes', async () => {
  const f = await planFixture(); f.provenance.binary.sha256 = 'f'.repeat(64);
  await writeFile(join(f.options.cli, 'build-provenance.json'), JSON.stringify(f.provenance));
  await expect(planAssembly(f.options)).rejects.toThrow('identity/target/binary');
});
test('plan rejects output paths whose parent symlink leads inside either input', async () => {
  const f = await planFixture();
  for (const source of [f.options.bundle, f.options.cli]) {
    const before = await inventory(source);
    const alias = join(await fixture(), 'input-alias');
    await symlink(source, alias);
    f.options.output = join(alias, 'new-output');
    await expect(planAssembly(f.options)).rejects.toThrow('outside inputs');
    expect(await inventory(source)).toEqual(before);
  }
});
test('assembly rejects a parent alias redirected into an input after planning', async () => {
  const f = await planFixture();
  const outside = await fixture(); const alias = join(await fixture(), 'output-alias');
  await symlink(outside, alias);
  f.options.output = join(alias, 'new-output');
  const plan = await planAssembly(f.options);
  plan.wizard = { status: 'passed', ...Object.fromEntries(['runtimeToolExecution', 'pythonDependencyInstallation', 'relocation', 'homeState'].map(k => [k, { status: 'passed' }])) };
  const before = await inventory(f.options.bundle);
  await rm(alias); await symlink(f.options.bundle, alias);
  await expect(assemble(f.options, plan)).rejects.toThrow('outside inputs');
  expect(await inventory(f.options.bundle)).toEqual(before);
});
test('assembly rejects a validator replaced after planning before reserving output', async () => {
  const f = await planFixture(); const plan = await planAssembly(f.options);
  plan.wizard = { status: 'passed', ...Object.fromEntries(['runtimeToolExecution', 'pythonDependencyInstallation', 'relocation', 'homeState'].map(k => [k, { status: 'passed' }])) };
  await writeFile(f.options.validator, '#!/bin/sh\nexit 0\n');
  await expect(assemble(f.options, plan)).rejects.toThrow('tool source changed after planning');
  await expect(lstat(f.options.output)).rejects.toMatchObject({ code: 'ENOENT' });
});

test('assembly requires every bound smoke check and has no skip switch', () => {
  expect(() => requireAssemblyProof({ wizard: { status: 'not-run' } })).toThrow('requires');
  const wizard = { status: 'passed', ...Object.fromEntries(['runtimeToolExecution', 'pythonDependencyInstallation', 'relocation', 'homeState'].map(k => [k, { status: 'passed' }])) };
  expect(() => requireAssemblyProof({ wizard })).not.toThrow();
  wizard.pythonDependencyInstallation.status = 'not-run';
  expect(() => requireAssemblyProof({ wizard })).toThrow('requires');
});
