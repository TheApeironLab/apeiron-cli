import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { version } from '../package.json';

// Publication uses GitHub OIDC credentials scoped to this OSS prefix. Never use root keys in CI.
const out = process.argv[2];
const repo = 'TheApeironLab/apeiron-cli';
const tag = `v${version}`;
const prefix = 'apeiron-cli';
const bucket = 'apeiron-bj-cli-downloads';
const base = `https://${bucket}.oss-cn-beijing.aliyuncs.com/${prefix}`;
if (!out || !isAbsolute(out) || process.env.GITHUB_REF !== `refs/tags/${tag}`) throw new Error('Publication requires a matching version tag and absolute artifact directory');
const hash = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');
async function run(args: string[]) {
  const child = Bun.spawn(args, { stdout: 'inherit', stderr: 'inherit' });
  if (await child.exited !== 0) throw new Error(`Failed: ${args[0]} ${args[1]}`);
}
const manifest = JSON.parse(await readFile(join(out, 'release.json'), 'utf8')) as { version: string; revision: string; files: Record<string, { sha256: string }> };
const revision = Bun.spawnSync(['git', 'rev-parse', `${tag}^{commit}`]);
if (revision.exitCode !== 0 || manifest.version !== version || revision.stdout.toString().trim() !== manifest.revision) throw new Error('Release source/tag mismatch');
const required = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64'].map(t => `apeiron-${version}-${t}.tar.gz`);
const names = [...required, 'install.sh', 'release.json', 'SHA256SUMS'];
const sums = new Map((await readFile(join(out, 'SHA256SUMS'), 'utf8')).trim().split('\n').map(line => {
  const match = /^([a-f0-9]{64})  ([A-Za-z0-9._-]+)$/.exec(line);
  if (!match) throw new Error('Malformed checksums');
  return [match[2]!, match[1]!];
}));
for (const name of names) {
  const bytes = await readFile(join(out, name));
  if (name !== 'SHA256SUMS' && hash(bytes) !== sums.get(name)) throw new Error(`Checksum mismatch: ${name}`);
}
async function publicBytes(url: string) {
  const response = await fetch(url, { redirect: 'error', signal: AbortSignal.timeout(180_000) });
  if (response.status === 404) return null;
  if (response.status !== 200) throw new Error(`Public download failed: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}
async function upload(name: string, key: string, immutable: boolean) {
  const path = join(out!, name);
  const bytes = await readFile(path);
  const url = `${base}/${key}`;
  const existing = await publicBytes(url);
  if (existing && hash(existing) === hash(bytes)) return;
  if (existing && immutable) throw new Error(`Immutable release object differs: ${key}`);
  await run(['aliyun', 'ossutil', 'cp', path, `oss://${bucket}/${prefix}/${key}`, '--region', 'cn-beijing', '--acl', 'public-read', '--no-progress',
    ...(immutable ? ['--ignore-existing'] : ['--force']), '--cache-control', immutable ? 'public,max-age=31536000,immutable' : 'no-cache', '--metadata', `sha256=${hash(bytes)}`]);
  const downloaded = await publicBytes(url);
  if (!downloaded || hash(downloaded) !== hash(bytes)) throw new Error(`Public checksum mismatch: ${key}`);
}
for (const name of names) await upload(name, `releases/${version}/${name}`, true);

const existing = Bun.spawnSync(['gh', 'release', 'view', tag, '--repo', repo, '--json', 'isDraft'], { stdout: 'pipe', stderr: 'pipe' });
if (existing.exitCode !== 0) {
  const notes = join(out, 'release-notes.md');
  await writeFile(notes, `Standalone macOS/Linux binaries for ARM64 and x64. No Bun or Node.js is required for apeiron init.\n\nInstall from the public OSS mirror:\n\n\`\`\`sh\ncurl -fsSL ${base}/install.sh | sh\napeiron init\n\`\`\`\n\nPreview: the bundled Chentu catalog currently supports local K3d ARM64. CLI binaries for other platforms can run the wizard but need a matching deployment bundle. GPUStack is not included in this catalog. macOS binaries are not Developer ID notarized.\n\nChecksums: SHA256SUMS. The GitHub source repository and releases remain private; OSS assets are public.\n`);
  await run(['gh', 'release', 'create', tag, '--repo', repo, '--verify-tag', '--draft', '--title', `Apeiron CLI ${version}`, '--notes-file', notes]);
}
// Re-runs can finish a draft/upload interrupted by network failure. Existing assets must match.
// The tag REST endpoint excludes drafts. Resolve the release ID first; gh's asset
// projection also omits digest in some versions, so read assets through REST.
const releaseResult = Bun.spawnSync(['gh', 'release', 'view', tag, '--repo', repo, '--json', 'apiUrl', '--jq', '.apiUrl']);
const apiUrl = releaseResult.stdout.toString().trim();
if (releaseResult.exitCode !== 0 || !apiUrl.startsWith(`https://api.github.com/repos/${repo}/releases/`)) throw new Error('Cannot resolve GitHub release ID');
const assetsResult = Bun.spawnSync(['gh', 'api', apiUrl, '--jq', '.assets']);
if (assetsResult.exitCode !== 0) throw new Error('Cannot inspect GitHub release assets');
const assets = JSON.parse(assetsResult.stdout.toString()) as { name: string; digest?: string }[];
for (const name of names) {
  const found = assets.find(a => a.name === name);
  if (found) {
    if (found.digest !== `sha256:${hash(await readFile(join(out, name)))}`) throw new Error(`GitHub asset differs or has no digest: ${name}`);
  } else await run(['gh', 'release', 'upload', tag, join(out, name), '--repo', repo]);
}
await run(['gh', 'release', 'edit', tag, '--repo', repo, '--draft=false', `--prerelease=${version.includes('-')}`]);
// Advance the mutable entry points only after every immutable download is verified.
await upload('install.sh', 'install.sh', false);
await writeFile(join(out, 'latest.txt'), `${version}\n`);
await upload('latest.txt', 'latest.txt', false);
console.log(`Published ${tag}: ${base}/install.sh`);
