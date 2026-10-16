import { createHash } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { version } from '../version';
// Publication targets for one finalized release directory (see docs/releases.md).
// `oss` is the public internet channel (OSS mirror + GitHub Releases); `forgejo`
// and `directory` serve intranets that cannot reach either.
export const publishTargets = ['oss', 'forgejo', 'directory'];
export const tag = `v${version}`;
export const ossBase = 'https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com/apeiron-cli';
const repository = 'TheApeironLab/apeiron-cli';
const bucket = 'apeiron-bj-cli-downloads';
const prefix = 'apeiron-cli';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
// Every immutable download is verified before the mutable entry points move;
// the same file set ships to every target so checksums are verified once.
const requiredNames = () => ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64']
    .map(t => `apeiron-${version}-${t}.tar.gz`).concat(['install.sh', 'release.json', 'SHA256SUMS']);
export async function run(args) {
    const child = Bun.spawn(args, { stdout: 'inherit', stderr: 'inherit' });
    if (await child.exited !== 0)
        throw new Error(`Failed: ${args[0]} ${args[1]}`);
}
// Validate the whole release before publishing any byte: the manifest must name
// this checkout's tag and revision, and every shipped file must match SHA256SUMS.
export async function verifiedRelease(out) {
    if (!out || !isAbsolute(out))
        throw new Error('Publication requires an absolute artifact directory');
    const manifest = JSON.parse(await readFile(join(out, 'release.json'), 'utf8'));
    const revision = Bun.spawnSync(['git', 'rev-parse', `${tag}^{commit}`]);
    if (revision.exitCode !== 0 || manifest.version !== version || revision.stdout.toString().trim() !== manifest.revision)
        throw new Error('Release source/tag mismatch');
    const names = requiredNames();
    const sums = new Map((await readFile(join(out, 'SHA256SUMS'), 'utf8')).trim().split('\n').map(line => {
        const match = /^([a-f0-9]{64})  ([A-Za-z0-9._-]+)$/.exec(line);
        if (!match)
            throw new Error('Malformed checksums');
        return [match[2], match[1]];
    }));
    for (const name of names) {
        const bytes = await readFile(join(out, name));
        if (name !== 'SHA256SUMS' && sha256(bytes) !== sums.get(name))
            throw new Error(`Checksum mismatch: ${name}`);
    }
    return names;
}
async function fetchJson(url, request) {
    const response = await fetch(url, { redirect: 'error', signal: AbortSignal.timeout(180_000), ...request });
    if (!response.ok)
        throw new Error(`HTTP ${response.status} for ${request?.method ?? 'GET'} ${url}`);
    return response.json();
}
// An already-present file is accepted only with identical bytes, so a rerun
// after a network failure can finish but never replace a published file.
async function writeVerified(path, bytes, immutable) {
    const existing = await readFile(path).then(current => current, error => {
        if (error.code !== 'ENOENT')
            throw error;
        return null;
    });
    if (existing && sha256(existing) === sha256(bytes))
        return;
    if (existing && immutable)
        throw new Error(`Immutable release file differs: ${path}`);
    const staged = `${path}.publish-${process.pid}`;
    await writeFile(staged, bytes);
    await rename(staged, path);
    if (sha256(await readFile(path)) !== sha256(bytes))
        throw new Error(`Written file differs: ${path}`);
}
// Publish the directory layout install.sh reads through APEIRON_DOWNLOAD_BASE:
// install.sh and latest.txt at the base, every tagged file under releases/<version>/.
export async function publishToDirectory(out, destination) {
    if (!destination || !isAbsolute(destination))
        throw new Error('Directory publication requires an absolute destination (APEIRON_PUBLISH_DIR)');
    const names = await verifiedRelease(out);
    const release = join(destination, 'releases', version);
    await mkdir(release, { recursive: true });
    for (const name of names)
        await writeVerified(join(release, name), await readFile(join(out, name)), true);
    // Mutable entry points move only after every immutable file is in place.
    await writeVerified(join(destination, 'install.sh'), await readFile(join(out, 'install.sh')), false);
    await writeVerified(join(destination, 'latest.txt'), new TextEncoder().encode(`${version}\n`), false);
    return destination;
}
// Upload the release as Forgejo release assets. The release carries the tag, so
// an intranet can pin a version (install.sh needs the flat layout instead — see
// publishToDirectory). The tag must already exist on the remote: publication
// never creates one, matching the GitHub path's --verify-tag.
export async function publishToForgejo(out, options) {
    const base = (options.base ?? '').replace(/\/+$/, '');
    const repo = options.repository;
    const token = options.token;
    if (!base || !repo || !token)
        throw new Error('Forgejo publication requires FORGEJO_BASE, FORGEJO_REPOSITORY and FORGEJO_TOKEN');
    if (!/^[^/]+\/[^/]+$/.test(repo))
        throw new Error('FORGEJO_REPOSITORY must be owner/name');
    const names = await verifiedRelease(out);
    const api = `${base}/api/v1/repos/${repo}`;
    const headers = { Authorization: `token ${token}` };
    if (!(await fetch(`${api}/tags/${tag}`, { headers, redirect: 'error', signal: AbortSignal.timeout(180_000) })).ok)
        throw new Error(`Forgejo remote is missing ${tag}; push the tag before publishing`);
    const notes = `Standalone macOS/Linux binaries for ARM64 and x64, checksum-verified against SHA256SUMS. Install from a mirror that serves the flat layout (install.sh, latest.txt, releases/${version}/) with APEIRON_DOWNLOAD_BASE pointing at it. Assets of this release are addressed per tag for version pinning.\n`;
    const existing = await fetchJson(`${api}/releases`, { headers });
    let release = existing.find(candidate => candidate.tag_name === tag);
    if (!release)
        release = await fetchJson(`${api}/releases`, {
            method: 'POST', headers: { ...headers, 'Content-Type': 'application/json' },
            body: JSON.stringify({ tag_name: tag, name: `Apeiron CLI ${version}`, draft: true, prerelease: version.includes('-'), body: notes }),
        });
    for (const name of names) {
        const bytes = await readFile(join(out, name));
        const asset = (release.assets ?? []).find(candidate => candidate.name === name);
        if (asset) {
            // Attachments carry no digest field across Forgejo versions, so the
            // only trustworthy comparison is the bytes themselves.
            const stored = await fetch(asset.browser_download_url, { headers, redirect: 'error', signal: AbortSignal.timeout(180_000) })
                .then(async response => response.ok ? new Uint8Array(await response.arrayBuffer()) : null)
                .catch(() => null);
            if (!stored)
                throw new Error(`Cannot verify Forgejo asset ${name}; remove it and rerun`);
            if (sha256(stored) !== sha256(bytes))
                throw new Error(`Forgejo asset differs: ${name}`);
            continue;
        }
        const form = new FormData();
        form.append('attachment', new Blob([bytes]), name);
        await fetchJson(`${api}/releases/${release.id}/assets?name=${encodeURIComponent(name)}`, { method: 'POST', headers, body: form });
    }
    await fetchJson(`${api}/releases/${release.id}`, {
        method: 'PATCH', headers: { ...headers, 'Content-Type': 'application/json' },
        body: JSON.stringify({ draft: false, prerelease: version.includes('-') }),
    });
    return `${base}/${repo}/releases/tag/${tag}`;
}
export async function publishToOss(out, env) {
    if (env.GITHUB_REF !== `refs/tags/${tag}`)
        throw new Error('Publication requires a matching version tag and absolute artifact directory');
    const names = await verifiedRelease(out);
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
        const url = `${ossBase}/${key}`;
        const existing = await publicBytes(url);
        if (existing && sha256(existing) === sha256(bytes))
            return;
        if (existing && immutable)
            throw new Error(`Immutable release object differs: ${key}`);
        await run(['aliyun', 'ossutil', 'cp', path, `oss://${bucket}/${prefix}/${key}`, '--region', 'cn-beijing', '--acl', 'public-read', '--no-progress',
            ...(immutable ? ['--ignore-existing'] : ['--force']), '--cache-control', immutable ? 'public,max-age=31536000,immutable' : 'no-cache', '--metadata', `sha256=${sha256(bytes)}`]);
        const downloaded = await publicBytes(url);
        if (!downloaded || sha256(downloaded) !== sha256(bytes))
            throw new Error(`Public checksum mismatch: ${key}`);
    }
    for (const name of names)
        await upload(name, `releases/${version}/${name}`, true);
    const existing = Bun.spawnSync(['gh', 'release', 'view', tag, '--repo', repository, '--json', 'isDraft'], { stdout: 'pipe', stderr: 'pipe' });
    if (existing.exitCode !== 0) {
        const notes = join(out, 'release-notes.md');
        await writeFile(notes, `Standalone macOS/Linux binaries for ARM64 and x64. No Bun or Node.js is required for apeiron init.\n\nInstall from the public OSS mirror:\n\n\`\`\`sh\ncurl -fsSL ${ossBase}/install.sh | sh\napeiron init\n\`\`\`\n\nPreview: the bundled Chentu catalog currently supports local K3d ARM64. CLI binaries for other platforms can run the wizard but need a matching deployment bundle. GPUStack is not included in this catalog. macOS binaries are not Developer ID notarized.\n\nChecksums: SHA256SUMS. The GitHub source repository and releases remain private; OSS assets are public.\n`);
        await run(['gh', 'release', 'create', tag, '--repo', repository, '--verify-tag', '--draft', '--title', `Apeiron CLI ${version}`, '--notes-file', notes]);
    }
    // Re-runs can finish a draft/upload interrupted by network failure. Existing assets must match.
    // The tag REST endpoint excludes drafts. Resolve the release ID first; gh's asset
    // projection also omits digest in some versions, so read assets through REST.
    const releaseResult = Bun.spawnSync(['gh', 'release', 'view', tag, '--repo', repository, '--json', 'apiUrl', '--jq', '.apiUrl']);
    const apiUrl = releaseResult.stdout.toString().trim();
    if (releaseResult.exitCode !== 0 || !apiUrl.startsWith(`https://api.github.com/repos/${repository}/releases/`))
        throw new Error('Cannot resolve GitHub release ID');
    const assetsResult = Bun.spawnSync(['gh', 'api', apiUrl, '--jq', '.assets']);
    if (assetsResult.exitCode !== 0)
        throw new Error('Cannot inspect GitHub release assets');
    const assets = JSON.parse(assetsResult.stdout.toString());
    for (const name of names) {
        const found = assets.find(a => a.name === name);
        if (found) {
            if (found.digest !== `sha256:${sha256(await readFile(join(out, name)))}`)
                throw new Error(`GitHub asset differs or has no digest: ${name}`);
        }
        else
            await run(['gh', 'release', 'upload', tag, join(out, name), '--repo', repository]);
    }
    await run(['gh', 'release', 'edit', tag, '--repo', repository, '--draft=false', `--prerelease=${version.includes('-')}`]);
    // Advance the mutable entry points only after every immutable download is verified.
    await upload('install.sh', 'install.sh', false);
    await writeFile(join(out, 'latest.txt'), `${version}\n`);
    await upload('latest.txt', 'latest.txt', false);
    return `${ossBase}/install.sh`;
}
export function targetFromEnv(env) {
    const target = env.APEIRON_PUBLISH_TARGET || 'oss';
    if (!publishTargets.includes(target))
        throw new Error(`Unknown publication target: ${target} (expected one of ${publishTargets.join(', ')})`);
    return target;
}
export async function publishRelease(out, target, env) {
    if (target === 'oss')
        return publishToOss(out, env);
    if (target === 'forgejo')
        return publishToForgejo(out, { base: env.FORGEJO_BASE, repository: env.FORGEJO_REPOSITORY, token: env.FORGEJO_TOKEN });
    return publishToDirectory(out, env.APEIRON_PUBLISH_DIR);
}
