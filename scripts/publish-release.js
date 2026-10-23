import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { version, channel, baseVersion } from './version';
import { mayAdvanceChannel } from './lib/release-version';
import { validateDistribution, validateAcceptanceSet } from './lib/distribution.js';
import { acceptanceScope } from './lib/acceptance-scope.js';
// Publication uses GitHub OIDC credentials scoped to this OSS prefix. Never use root keys in CI.
const out = process.argv[2];
const repo = 'TheApeironLab/apeiron-cli';
const tag = `v${version}`;
const prefix = 'apeiron-cli';
const bucket = 'apeiron-bj-cli-downloads';
const base = `https://downloads.apeironlab.cn/${prefix}`;
if (!out || !isAbsolute(out) || process.env.GITHUB_REF !== `refs/tags/${tag}`)
    throw new Error('Publication requires a matching version tag and absolute artifact directory');
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
async function run(args) {
    const child = Bun.spawn(args, { stdout: 'inherit', stderr: 'inherit' });
    if (await child.exited !== 0)
        throw new Error(`Failed: ${args[0]} ${args[1]}`);
}
const manifest = JSON.parse(await readFile(join(out, 'release.json'), 'utf8'));
const revision = Bun.spawnSync(['git', 'rev-parse', `${tag}^{commit}`]);
if (revision.exitCode !== 0 || manifest.version !== version || manifest.baseVersion !== baseVersion
    || manifest.releaseChannel !== channel || revision.stdout.toString().trim() !== manifest.revision)
    throw new Error('Release source/tag mismatch');
const distribution = validateDistribution(JSON.parse(await readFile(join(out, 'distribution.json'), 'utf8')), manifest);
if (distribution.cli?.revision !== manifest.revision || JSON.stringify(distribution.cli.files) !== JSON.stringify(manifest.files))
    throw new Error('Distribution CLI artifacts mismatch');
validateAcceptanceSet(distribution.acceptance, manifest, distribution.inputSha256, process.env.GITHUB_RUN_ID);
const required = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64'].map(t => `apeiron-${version}-${t}.tar.gz`);
const names = [...required, 'install.sh', 'release.json', 'distribution.json', 'SHA256SUMS'];
const sums = new Map((await readFile(join(out, 'SHA256SUMS'), 'utf8')).trim().split('\n').map(line => {
    const match = /^([a-f0-9]{64})  ([A-Za-z0-9._-]+)$/.exec(line);
    if (!match)
        throw new Error('Malformed checksums');
    return [match[2], match[1]];
}));
for (const name of names) {
    const bytes = await readFile(join(out, name));
    if (name !== 'SHA256SUMS' && hash(bytes) !== sums.get(name))
        throw new Error(`Checksum mismatch: ${name}`);
    if ((required.includes(name) || name === 'install.sh') && (manifest.files?.[name]?.sha256 !== hash(bytes)
        || manifest.files[name].size !== bytes.length))
        throw new Error(`Release manifest artifact mismatch: ${name}`);
}
async function publicBytes(url) {
    const response = await fetch(url, { redirect: 'error', signal: AbortSignal.timeout(180_000) });
    if (response.status === 404)
        return null;
    if (response.status !== 200)
        throw new Error(`Public download failed: HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
}
async function upload(name, key, immutable) {
    const path = join(out, name);
    const bytes = await readFile(path);
    const url = `${base}/${key}`;
    const existing = await publicBytes(url);
    if (existing && hash(existing) === hash(bytes))
        return;
    if (existing && immutable)
        throw new Error(`Immutable release object differs: ${key}`);
    await run(['aliyun', 'ossutil', 'cp', path, `oss://${bucket}/${prefix}/${key}`, '--region', 'cn-beijing', '--acl', 'public-read', '--no-progress',
        ...(immutable ? ['--ignore-existing'] : ['--force']), '--cache-control', immutable ? 'public,max-age=31536000,immutable' : 'no-cache', '--metadata', `sha256=${hash(bytes)}`]);
    const downloaded = await publicBytes(url);
    if (!downloaded || hash(downloaded) !== hash(bytes))
        throw new Error(`Public checksum mismatch: ${key}`);
}
for (const name of names)
    await upload(name, `releases/${version}/${name}`, true);
const existing = Bun.spawnSync(['gh', 'release', 'view', tag, '--repo', repo, '--json', 'isDraft'], { stdout: 'pipe', stderr: 'pipe' });
if (existing.exitCode !== 0) {
    const notes = join(out, 'release-notes.md');
    const scope = acceptanceScope(version);
    const coverage = `Installation acceptance covers ${scope.applications.length} applications on all four declared deployment targets. `
      + (scope.deferredApplications.length ? `Deferred acceptance: ${scope.deferredApplications.join(', ')} remains packaged but is not part of this nightly's release acceptance. ` : '')
      + 'This does not certify every application business journey, GPU inference, failover or disconnected installation. See distribution.json for the bound acceptance receipts.';
    await writeFile(notes, `Apeiron CLI ${version} (${channel}). Standalone macOS/Linux binaries for ARM64 and x64. No Bun or Node.js is required for apeiron init.\n\n${coverage}\n\nInstall this exact version from the public OSS mirror:\n\n\`\`\`sh\ncurl -fsSL ${base}/releases/${version}/install.sh | sh -s -- --version ${version}\napeiron init\n\`\`\`\n\nCLI platform support does not imply a matching deployment bundle exists for every platform. macOS binaries are not Developer ID notarized.\n\nChecksums: SHA256SUMS. The GitHub source repository and releases remain private; OSS assets are public.\n`);
    await run(['gh', 'release', 'create', tag, '--repo', repo, '--verify-tag', '--draft', '--title', `Apeiron CLI ${version}`, '--notes-file', notes]);
}
// Re-runs can finish a draft/upload interrupted by network failure. Existing assets must match.
// The tag REST endpoint excludes drafts. Resolve the release ID first; gh's asset
// projection also omits digest in some versions, so read assets through REST.
const releaseResult = Bun.spawnSync(['gh', 'release', 'view', tag, '--repo', repo, '--json', 'apiUrl', '--jq', '.apiUrl']);
const apiUrl = releaseResult.stdout.toString().trim();
if (releaseResult.exitCode !== 0 || !apiUrl.startsWith(`https://api.github.com/repos/${repo}/releases/`))
    throw new Error('Cannot resolve GitHub release ID');
const assetsResult = Bun.spawnSync(['gh', 'api', apiUrl, '--jq', '.assets']);
if (assetsResult.exitCode !== 0)
    throw new Error('Cannot inspect GitHub release assets');
const assets = JSON.parse(assetsResult.stdout.toString());
for (const name of names) {
    const found = assets.find(a => a.name === name);
    if (found) {
        if (found.digest !== `sha256:${hash(await readFile(join(out, name)))}`)
            throw new Error(`GitHub asset differs or has no digest: ${name}`);
    }
    else
        await run(['gh', 'release', 'upload', tag, join(out, name), '--repo', repo]);
}
// The workflow serializes ALL publishers, including different tags. Re-read the
// channel while holding that job-level lock so a late old build cannot move it back.
const pointer = await publicBytes(`${base}/channels/${channel}.txt`);
const current = pointer === null ? null : new TextDecoder().decode(pointer).trim();
const advance = mayAdvanceChannel(version, current, channel);
await run(['gh', 'release', 'edit', tag, '--repo', repo, '--draft=false',
    `--prerelease=${channel === 'nightly'}`, `--latest=${channel === 'stable' && advance}`]);
if (advance) {
    // All immutable objects are verified first. A same-version retry also repairs
    // an interrupted stable/install/latest update; nightly never touches those aliases.
    await writeFile(join(out, 'channel.txt'), `${version}\n`);
    await upload('channel.txt', `channels/${channel}.txt`, false);
    if (channel === 'stable') {
        await upload('install.sh', 'install.sh', false);
        await upload('channel.txt', 'latest.txt', false);
    }
} else {
    console.log(`Kept newer ${channel} pointer ${current}`);
}
console.log(`Published ${tag} (${channel}): ${base}/releases/${version}/`);
