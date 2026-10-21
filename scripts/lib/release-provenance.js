import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

export const releaseTargets = {
  'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-musl', 'linux-x64': 'x86_64-unknown-linux-musl',
};
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
function output(args, root) {
  const result = Bun.spawnSync(args, { cwd: root, stdout: 'pipe', stderr: 'pipe' });
  if (result.exitCode !== 0) throw new Error('Cannot inspect release source: ' + args[0]);
  return result.stdout.toString().trim();
}
export async function sourceSnapshot(root) {
  if (output(['git', 'status', '--porcelain', '--untracked-files=all', '--ignore-submodules=none'], root))
    throw new Error('Release builds require a clean committed checkout (including untracked files)');
  const submodules = Bun.spawnSync(['git', 'submodule', 'status', '--recursive'], { cwd: root, stdout: 'pipe', stderr: 'pipe' });
  if (submodules.exitCode !== 0 || submodules.stdout.toString().split('\n').filter(Boolean).some(line => !line.startsWith(' ')))
    throw new Error('Release submodules must be initialized at their pinned commits; run git submodule update --init --recursive');
  const revision = output(['git', 'rev-parse', 'HEAD'], root);
  const tree = output(['git', 'rev-parse', 'HEAD^{tree}'], root);
  if (!/^[a-f0-9]{40}$/.test(revision) || !/^[a-f0-9]{40}$/.test(tree))
    throw new Error('Release source must be an exact Git commit and tree');
  const lock = await readFile(join(root, 'Cargo.lock'));
  const toolchain = await readFile(join(root, 'rust-toolchain.toml'));
  const channel = toolchain.toString().match(/^channel\s*=\s*"([^"]+)"/m)?.[1];
  if (!/^\d+\.\d+\.\d+$/.test(channel || '')) throw new Error('Release Rust toolchain must be pinned');
  return { revision, tree, cargoLockSha256: sha256(lock), rustToolchainSha256: sha256(toolchain), channel };
}
export function assertSourceUnchanged(before, after) {
  if (JSON.stringify(before) !== JSON.stringify(after))
    throw new Error('Source changed during release build; refusing to label the artifact');
}
export function buildProvenance(source, version, target, binary, rustc, cargo) {
  if (!releaseTargets[target] || !rustc.startsWith('rustc ' + source.channel + ' '))
    throw new Error('Release compiler or target does not match the pinned toolchain');
  return {
    schemaVersion: 1, kind: 'apeiron-cli-build', version, target, triple: releaseTargets[target],
    source, toolchain: { channel: source.channel, rustc, cargo, bun: Bun.version },
    binary: { size: binary.length, sha256: sha256(binary) },
  };
}
export function validateProvenance(value, source, version, target, binary) {
  if (value?.schemaVersion !== 1 || value.kind !== 'apeiron-cli-build' || value.version !== version
      || value.target !== target || value.triple !== releaseTargets[target])
    throw new Error('Build provenance version/target mismatch');
  assertSourceUnchanged(source, value.source);
  if (value.toolchain?.channel !== source.channel
      || !value.toolchain.rustc?.startsWith('rustc ' + source.channel + ' ')
      || !value.toolchain.cargo?.startsWith('cargo '))
    throw new Error('Build provenance toolchain mismatch');
  if (value.binary?.size !== binary.length || value.binary.sha256 !== sha256(binary))
    throw new Error('Build provenance binary mismatch');
}
export function compilerVersion(tool, root) { return output([tool, '-vV'], root); }
export async function archiveMember(archive, member) {
  const child = Bun.spawn(['tar', '-xOf', archive, member], { stdout: 'pipe', stderr: 'pipe' });
  const [bytes, diagnostic, code] = await Promise.all([
    new Response(child.stdout).arrayBuffer(), new Response(child.stderr).text(), child.exited,
  ]);
  if (code) throw new Error('Release archive is missing ' + member);
  return Buffer.from(bytes);
}
export async function inspectReleaseArtifact(path, source, version, target) {
  const bytes = await readFile(path);
  const listing = Bun.spawnSync(['tar', '-tf', path], { stdout: 'pipe', stderr: 'pipe' });
  const detailed = Bun.spawnSync(['tar', '-tvf', path], { stdout: 'pipe', stderr: 'pipe' });
  const members = listing.stdout.toString().trim().split('\n').sort();
  if (listing.exitCode || detailed.exitCode
      || JSON.stringify(members) !== JSON.stringify(['README.md', 'apeiron', 'build-provenance.json'])
      || detailed.stdout.toString().trim().split('\n').some(line => !line.startsWith('-')))
    throw new Error('Release archive must contain exactly three regular files');
  const provenance = JSON.parse((await archiveMember(path, 'build-provenance.json')).toString());
  validateProvenance(provenance, source, version, target, await archiveMember(path, 'apeiron'));
  const sidecar = JSON.parse(await readFile(path + '.provenance.json', 'utf8'));
  if (sidecar.archive?.size !== bytes.length || sidecar.archive.sha256 !== sha256(bytes))
    throw new Error('Archive provenance checksum mismatch');
  const { archive, ...built } = sidecar;
  if (JSON.stringify(built) !== JSON.stringify(provenance))
    throw new Error('Archive provenance differs from sidecar');
  return provenance;
}
