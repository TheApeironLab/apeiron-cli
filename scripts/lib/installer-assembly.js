import { validateProvenance } from './release-provenance.js';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, readdir, readFile, writeFile, mkdir, copyFile, chmod, realpath } from 'node:fs/promises';
import { join, resolve, isAbsolute, dirname, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

export const digest = value => createHash('sha256').update(value).digest('hex');
const json = value => JSON.stringify(value, null, 2) + '\n';
const toolRoot = fileURLToPath(new URL('../../', import.meta.url));
const assemblyToolPaths = ['scripts/assemble-installer.js', 'scripts/lib/installer-assembly.js', 'scripts/lib/release-provenance.js', 'scripts/lib/verify-installer-archive.py', 'scripts/test-installer-wizard.py'];
async function toolSources(validator) {
  const files = [];
  for (const path of assemblyToolPaths) files.push({ path, sha256: await hashFile(join(toolRoot, path)) });
  return { validator: { path: validator, sha256: await hashFile(validator) }, assembly: { kind: 'working-tree-tool-source', files } };
}
export function safePath(path) {
  if (!path || isAbsolute(path) || path.includes('\\') || /[\x00-\x1f\x7f]/.test(path) || path.split('/').some(p => !p || p === '.' || p === '..')) throw new Error('Unsafe package path');
  return path;
}
export async function hashFile(path) {
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  return hash.digest('hex');
}
export async function inventory(root) {
  if (!(await lstat(root)).isDirectory()) throw new Error('Input must be a real directory');
  const files = [];
  async function visit(prefix) {
    for (const name of (await readdir(join(root, prefix))).sort()) {
      const path = prefix ? prefix + '/' + name : name;
      safePath(path);
      const stat = await lstat(join(root, path));
      if (stat.isDirectory()) await visit(path);
      else if (stat.isFile() && stat.nlink === 1) files.push({ path, size: stat.size, mode: stat.mode & 0o777, sha256: await hashFile(join(root, path)) });
      else throw new Error('Only regular files and directories allowed: ' + path);
    }
  }
  await visit('');
  return files;
}
export async function verifyChecksums(root, files) {
  const sums = await readFile(join(root, 'SHA256SUMS'), 'utf8');
  const seen = new Set();
  const actual = new Map(files.map(f => [f.path, f]));
  for (const line of sums.trim().split('\n')) {
    const match = line.match(/^([a-f0-9]{64}) [ *](.+)$/);
    if (!match) throw new Error('Malformed SHA256SUMS');
    const path = safePath(match[2].replace(/^\.\//, ''));
    if (seen.has(path)) throw new Error('Duplicate checksum path: ' + path);
    seen.add(path);
    if (actual.get(path)?.sha256 !== match[1]) throw new Error('Missing or changed resource: ' + path);
  }
  const unlisted = files.filter(f => f.path !== 'SHA256SUMS' && !seen.has(f.path));
  if (unlisted.length) throw new Error('Unlisted payload files: ' + unlisted.map(f => f.path).slice(0, 5).join(', '));
}
export function checkProfile(profile) {
  if (!['legacy-native', 'hosted-first'].includes(profile.contract)) throw new Error('Unknown installation contract');
  const config = profile.config;
  const d = config?.deployment;
  const i = d?.installation;
  const topology = i?.topology;
  // Legacy packages only ship multi-k3s; hosted-first packages ship all three topologies.
  const topologies = profile.contract === 'legacy-native' ? ['multi-k3s'] : ['multi-k3s', 'single-k3s', 'single-k3d'];
  if (d?.offline !== true || !topologies.includes(topology)) throw new Error('Profile must use an offline ' + topologies.join('/') + ' configuration');
  if (d.runner !== (topology === 'single-k3d' ? 'docker' : 'native')) throw new Error('Runner must match the topology (docker for single-k3d, native otherwise)');
  const nodes = i.nodes;
  if (!Array.isArray(nodes)) throw new Error('Missing nodes');
  if (topology === 'multi-k3s') {
    const roles = profile.contract === 'legacy-native' ? ['agent', 'agent', 'server'] : ['server', 'server', 'server'];
    if (nodes.length !== 3 || JSON.stringify(nodes.map(n => n.role).sort()) !== JSON.stringify(roles)) throw new Error('Wrong node roles for installation contract');
    if (nodes.some(n => !/^192\.0\.2\.\d+$/.test(n.host || '') || n.address !== n.host)) throw new Error('Default nodes must use documentation placeholders');
  } else if (nodes.length) throw new Error('Single-host topologies take no nodes');
  if (topology === 'single-k3d') {
    if (i.entryIp !== '127.0.0.1' || i.httpPort !== 54320 || i.httpsPort !== 54321) throw new Error('single-k3d defaults must use 127.0.0.1 with ports 54320/54321');
  } else if (i.sshKey || i.sshUser || i.entryIp) throw new Error('Default configuration must not include access credentials or live addresses');
  if (config.models?.apiKey || config.llm?.apiKey) throw new Error('Default configuration must not include model API credentials');
  if (!Array.isArray(config.apps) || !config.apps.length) throw new Error('Profile applications required');
  return profile;
}
async function outputDestination(output, inputs) {
  const absolute = resolve(output);
  let ancestor = dirname(absolute);
  for (;;) {
    try {
      await lstat(ancestor);
      break;
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      const parent = dirname(ancestor);
      if (parent === ancestor) throw error;
      ancestor = parent;
    }
  }
  const destination = resolve(await realpath(ancestor), relative(ancestor, absolute));
  for (const input of inputs) {
    const source = await realpath(input);
    if (destination === source || destination.startsWith(source + '/')) throw new Error('Output must be outside inputs');
  }
  return destination;
}
export async function credentialCheck(root, files) {
  for (const f of files) {
    if (/(^|\/)(\.env(?:\..+)?|id_rsa|id_ed25519|credentials|kubeconfig)$/.test(f.path)) throw new Error('Potential credential file: ' + f.path);
    if (f.size < 2_000_000 && /\.(?:json|ya?ml|sh|txt|md|pem|key)$/.test(f.path)) {
      const value = await readFile(join(root, f.path), 'utf8');
      if (/-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----/.test(value) || /(?:AKIA|ASIA)[A-Z0-9]{16}/.test(value)) throw new Error('Potential credential content: ' + f.path);
    }
  }
}
function run(args, cwd) {
  const result = Bun.spawnSync(args, { cwd, stdout: 'pipe', stderr: 'pipe', env: { ...process.env, COPYFILE_DISABLE: '1' } });
  if (result.exitCode !== 0) throw new Error('Validation command failed: ' + args[0] + '\n' + result.stderr.toString().slice(0, 4000));
  return result.stdout.toString();
}
export function startScript(candidate, pythonArchive, contract = {}) {
  safePath(pythonArchive);
  if (!/^python\/[A-Za-z0-9._+-]+\.tar\.gz$/.test(pythonArchive)) throw new Error('Unsafe Python archive name');
  const base = (contract.baseComponents || []).join(', ');
  if (!/^[a-zA-Z0-9_, -]*$/.test(base)) throw new Error('Unsafe base component names');
  const notice = contract.contract === 'hosted-first' ? `printf '%s\\n' 'Hosted mandatory base (${(contract.baseComponents || []).length} components): ${base}' 'Application selection does not narrow this mandatory base.'\n` : '';
  return `#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BUNDLE="$HERE/bundle"
[[ "$(uname -s)" = Linux && "$(uname -m)" = x86_64 ]] || { echo 'Requires Ubuntu 22.04 AMD64' >&2; exit 2; }
. /etc/os-release
[[ "$ID" = ubuntu && "$VERSION_ID" = 22.04 ]] || { echo 'Requires Ubuntu 22.04 AMD64' >&2; exit 2; }
for tool in ssh tar gpg; do command -v "$tool" >/dev/null || { echo "Missing host prerequisite: $tool" >&2; exit 2; }; done
umask 077
INSTANCE="\${APEIRON_INSTANCE:-default}"
[[ "$INSTANCE" =~ ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$ ]] || { echo 'APEIRON_INSTANCE must be a lowercase DNS label' >&2; exit 2; }
WIZARD_PORT="\${APEIRON_WIZARD_PORT:-3210}"
[[ "$WIZARD_PORT" =~ ^[1-9][0-9]{0,4}$ && "$WIZARD_PORT" -le 65535 ]] || { echo 'APEIRON_WIZARD_PORT must be 1-65535' >&2; exit 2; }
ROOT="$HOME/.apeiron/installers/${candidate}"
STATE="$ROOT/instances/$INSTANCE"
mkdir -p "$ROOT/bootstrap" "$STATE/cache" "$STATE/config" "$STATE/data"
if [[ ! -x "$ROOT/bootstrap/python/bin/python3.12" ]]; then
  tar -xzf "$BUNDLE/${pythonArchive}" -C "$ROOT/bootstrap"
fi
export PATH="$ROOT/bootstrap/python/bin:$BUNDLE/bin:$PATH"
export PYTHONDONTWRITEBYTECODE=1 PYTHONPYCACHEPREFIX="$STATE/cache/python"
export XDG_CACHE_HOME="$STATE/cache" XDG_CONFIG_HOME="$STATE/config" XDG_DATA_HOME="$STATE/data"
export K3S_DATA_DIR="$STATE/cache/k3s" UV_CACHE_DIR="$STATE/cache/uv"
export HELM_CACHE_HOME="$STATE/cache/helm" HELM_CONFIG_HOME="$STATE/config/helm" HELM_DATA_HOME="$STATE/data/helm"
python3.12 - "$STATE/config.json" "$BUNDLE" "$HERE/provenance/default-config.json" <<'PY'
import json, os, pathlib, sys
p = pathlib.Path(sys.argv[1])
value = json.loads(p.read_text() if p.exists() else pathlib.Path(sys.argv[3]).read_text())
value['deployment']['bundleDir'] = str(pathlib.Path(sys.argv[2]).resolve())
value['deployment']['offline'] = True
p.write_text(json.dumps(value, indent=2) + '\\n')
p.chmod(0o600)
PY
${notice}printf 'Configure the placeholder nodes and organization in the browser. Deployment requires an explicit action.\\n'
printf 'Instance %s: state in %s (APEIRON_INSTANCE selects another instance; APEIRON_WIZARD_PORT changes the wizard port).\\n' "$INSTANCE" "$STATE"
printf 'Remote access: run  ssh -L %s:127.0.0.1:%s <user>@<this-host>  on your workstation, then open the printed url as http://127.0.0.1:%s/...\\n' "$WIZARD_PORT" "$WIZARD_PORT" "$WIZARD_PORT"
exec "$HERE/bin/apeiron" init --config "$STATE/config.json" --port "$WIZARD_PORT" "$@"
`;
}
export async function planAssembly(options) {
  const { bundle, cli, output, candidate, profile: profilePath, validator } = options;
  if (!/^[a-zA-Z0-9][a-zA-Z0-9._-]{0,79}$/.test(candidate || '')) throw new Error('Invalid candidate identifier');
  for (const p of [bundle, cli, output, profilePath, validator]) if (!p || !isAbsolute(p)) throw new Error('All input and output paths must be absolute');
  try { await lstat(output); throw new Error('Output already exists'); } catch (e) { if (e.code !== 'ENOENT') throw e; }
  const canonicalOutput = await outputDestination(output, [bundle, cli]);
  const sources = await toolSources(validator);
  const profileBytes = await readFile(profilePath);
  const profile = checkProfile(JSON.parse(profileBytes));
  const files = await inventory(bundle);
  await verifyChecksums(bundle, files);
  await credentialCheck(bundle, files);
  const cliFiles = await inventory(cli);
  const provenance = JSON.parse(await readFile(join(cli, 'build-provenance.json')));
  const binary = cliFiles.find(f => f.path === 'apeiron');
  if (provenance.kind !== 'apeiron-cli-build' || provenance.schemaVersion !== 1 || provenance.target !== 'linux-x64' || provenance.triple !== 'x86_64-unknown-linux-musl' || !/^[a-f0-9]{40}$/.test(provenance.source?.revision || '') || !/^[a-f0-9]{40}$/.test(provenance.source?.tree || '') || binary?.sha256 !== provenance.binary?.sha256 || binary?.size !== provenance.binary?.size) throw new Error('CLI build identity/target/binary mismatch');
  const binaryBytes = await readFile(join(cli, 'apeiron'));
  validateProvenance(provenance, provenance.source, provenance.version, 'linux-x64', binaryBytes);
  const elf = binaryBytes.subarray(0, 20);
  if (elf.toString('hex', 0, 4) !== '7f454c46' || elf[4] !== 2 || elf[5] !== 1 || elf.readUInt16LE(18) !== 62) throw new Error('CLI must be a Linux AMD64 ELF binary');
  // The development validator reads the supplied default config and real CLI parsers.
  const contract = JSON.parse(run([validator, '--bundle', bundle, '--config', profilePath]));
  if (JSON.stringify(await toolSources(validator)) !== JSON.stringify(sources)) throw new Error('Assembly or validator tool source changed during planning');
  if (contract.contract !== profile.contract || contract.target?.arch !== 'amd64' || contract.target?.ubuntu !== 'jammy') throw new Error('Validator contract/platform mismatch');
  const expected = contract.expectedCli || profile.expectedCli;
  if (!expected || (expected.commit || expected.revision) !== provenance.source.revision || contract.validator?.sourceRevision !== provenance.source.revision) throw new Error('CLI and validator must match the declared source identity');
  if (expected.binary && (expected.binary.sha256 !== binary.sha256 || expected.binary.size !== binary.size)) throw new Error('Declared CLI binary mismatch');
  const indexed = new Map(files.map(f => [f.path, f]));
  if (!Array.isArray(contract.files) || !contract.files.length) throw new Error('Validator returned no resource closure');
  for (const file of contract.files) {
    const actual = indexed.get(safePath(file.path));
    if (!actual || actual.sha256 !== file.sha256 || actual.size !== file.size) throw new Error('Resource contract mismatch: ' + file.path);
  }
  const python = files.filter(f => /^python\/[^/]+\.tar\.gz$/.test(f.path));
  if (python.length !== 1) throw new Error('Expected exactly one bundled Python archive');
  const bindings = { bundleInventorySha256: digest(json(files)), cliSha256: binary.sha256, profileSha256: digest(profileBytes), startScriptSha256: digest(startScript(candidate, python[0].path, contract)), defaultConfigSha256: digest(json(profile.config)) };
  const declaredSource = contract.bundleSource || profile.bundleSource;
  if (!/^[a-f0-9]{40}$/.test(declaredSource?.commit || '')) throw new Error('Exact bundle source commit required in declaration or legacy profile');
  if (profile.bundleSource && contract.bundleSource && profile.bundleSource.commit !== contract.bundleSource.commit) throw new Error('Bundle source declaration mismatch');
  if (!contract.runtimeTools?.length || contract.checks?.runtimeTools !== 'passed') throw new Error('Runtime tool closure not verified by actual CLI validator');
  let wizard = { status: 'not-run' };
  if (options.wizardProof) {
    wizard = JSON.parse(await readFile(options.wizardProof));
    if (wizard.status !== 'passed' || !wizard.page || !wizard.configuration || Object.keys(bindings).some(k => wizard.bindings?.[k] !== bindings[k])) throw new Error('Wizard proof does not match inputs or required checks');
    if (wizard.buildInfo?.revision !== provenance.source.revision || wizard.buildInfo?.version !== provenance.version || wizard.buildInfo?.platform !== 'linux' || wizard.buildInfo?.architecture !== 'x86_64' || wizard.buildInfo?.clean !== true) throw new Error('Actual CLI build-info does not match provenance');
  }
  return { schemaVersion: 1, kind: 'apeiron-installer-assembly-plan', candidate, contract, profile, bindings, cliProvenance: provenance, bundleSource: declaredSource, toolSources: sources, files, cliFiles, pythonArchive: python[0].path, wizard, sourceLocations: { bundle, cli }, output: canonicalOutput };
}
async function copyInventory(source, destination, files) {
  for (const file of files) {
    const target = join(destination, file.path);
    await mkdir(dirname(target), { recursive: true });
    await copyFile(join(source, file.path), target);
    await chmod(target, file.mode);
  }
}
export function requireAssemblyProof(plan) {
  if (plan.wizard?.status !== 'passed' || !['runtimeToolExecution', 'pythonDependencyInstallation', 'relocation', 'homeState'].every(key => plan.wizard[key]?.status === 'passed')) throw new Error('Assembly requires bound wizard, tool execution, offline Python dependencies, relocation and home-state checks');
}
export async function assemble(options, plan) {
  requireAssemblyProof(plan);
  const output = await outputDestination(options.output, [options.bundle, options.cli]);
  if (output !== plan.output) throw new Error('Output location changed after planning');
  if (JSON.stringify(await toolSources(options.validator)) !== JSON.stringify(plan.toolSources)) throw new Error('Assembly or validator tool source changed after planning');
  // Exclusive creation reserves the output; failures retain evidence and never overwrite it.
  await mkdir(output);
  const root = join(output, options.candidate);
  await mkdir(join(root, 'provenance'), { recursive: true });
  await mkdir(join(root, 'bin'));
  await copyInventory(options.bundle, join(root, 'bundle'), plan.files);
  await copyFile(join(options.cli, 'apeiron'), join(root, 'bin/apeiron'));
  await chmod(join(root, 'bin/apeiron'), 0o755);
  await writeFile(join(root, 'provenance/cli-build-provenance.json'), json(plan.cliProvenance));
  await writeFile(join(root, 'provenance/default-config.json'), json(plan.profile.config));
  await writeFile(join(root, 'provenance/assembly.json'), json(plan));
  await writeFile(join(root, 'start.sh'), startScript(options.candidate, plan.pythonArchive, plan.contract), { mode: 0o755 });
  run(['bash', '-n', join(root, 'start.sh')]);
  const copied = await inventory(join(root, 'bundle'));
  if (JSON.stringify(copied) !== JSON.stringify(plan.files)) throw new Error('Copied payload differs from verified input');
  if (await hashFile(join(root, 'bin/apeiron')) !== plan.bindings.cliSha256) throw new Error('Copied CLI changed');
  if (JSON.stringify(await inventory(options.bundle)) !== JSON.stringify(plan.files) || JSON.stringify(await inventory(options.cli)) !== JSON.stringify(plan.cliFiles) || await hashFile(options.profile) !== plan.bindings.profileSha256) throw new Error('Input changed during assembly');
  const validation = { schemaVersion: 1, assembly: { status: 'passed' }, resources: { status: 'passed', count: plan.contract.files.length }, startupSyntax: { status: 'passed' }, runtimeTools: { status: 'passed', scope: 'static required-file closure' }, runtimeToolExecution: plan.wizard.runtimeToolExecution || { status: 'not-run' }, pythonDependencyInstallation: plan.wizard.pythonDependencyInstallation || { status: 'not-run' }, credentials: { status: 'passed', scope: 'file-name and private-key/token signatures; not a general secret audit' }, wizard: plan.wizard, actualDeployment: { status: 'not-run' }, disconnectedDeployment: { status: 'not-run' }, login: { status: 'not-run' }, business: { status: 'not-run' }, archive: { status: 'verified-by-external-receipt', receipt: '../archive-validation.json' } };
  await writeFile(join(root, 'validation.json'), json(validation));
  await writeFile(join(root, 'README.md'), `# ${options.candidate}\n\nLocal candidate, not a published release. Contract: ${plan.contract.contract}.\n\nRun ./start.sh on Ubuntu 22.04 AMD64. Host prerequisites: bash, ssh, tar, gpg. The bundled Python runtime is prepared in your home directory. For headless access run \`ssh -L 3210:127.0.0.1:3210 <user>@<host>\` from your workstation, start with --no-open, and open the printed URL via http://127.0.0.1:3210/. Configure the placeholder addresses before explicitly starting deployment.\n\nOffline mode is the default. Candidate state is isolated under ~/.apeiron/installers/${options.candidate}; each installation instance keeps its own config under instances/<name>/ (APEIRON_INSTANCE, default \`default\`; APEIRON_WIZARD_PORT changes the wizard port). Moving the package updates its resource path when start.sh runs.\n\nComponent closure: ${(plan.contract.componentClosure || []).join(', ')}. Hosted base: ${(plan.contract.baseComponents || []).join(', ')}.\n\nSee provenance/assembly.json and validation.json. Native deployment, disconnected deployment, login and business acceptance have not been executed by this assembly. Existing hosted image-pin deployment failures are not repaired by packaging.\n`);
  const finalFiles = await inventory(root);
  await writeFile(join(root, 'SHA256SUMS'), finalFiles.map(f => `${f.sha256}  ${f.path}\n`).join(''));
  const archive = join(output, options.candidate + '.tar');
  run(['tar', '-cf', archive, '-C', output, options.candidate]);
  const helper = fileURLToPath(new URL('./verify-installer-archive.py', import.meta.url));
  const result = JSON.parse(run(['python3', helper, archive, options.candidate]));
  const receipt = { ...result, archive: { path: options.candidate + '.tar', size: (await lstat(archive)).size, sha256: await hashFile(archive) } };
  await writeFile(join(output, 'archive-validation.json'), json(receipt));
  await writeFile(join(output, 'SHA256SUMS'), `${receipt.archive.sha256}  ${options.candidate}.tar\n${await hashFile(join(output, 'archive-validation.json'))}  archive-validation.json\n`);
  return receipt;
}
