import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { parseReleaseVersion } from './release-version.js';

export async function fileIdentity(path) {
  const hash = createHash('sha256');
  const info = await lstat(path);
  if (!info.isFile()) throw new Error('Release inputs must be ordinary files');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return { sha256: hash.digest('hex'), size: info.size };
}

// HEAD trusts the sha256 metadata that the uploader set and then read back in full.
export async function publicIdentity(url, method = 'GET') {
  const response = await fetch(url, { method, redirect: 'error', signal: AbortSignal.timeout(3_600_000) });
  if (response.status === 404) return null;
  if (response.status !== 200 || !response.body && method === 'GET') throw new Error(`Cannot verify public artifact: HTTP ${response.status}`);
  if (method === 'HEAD') return { sha256: response.headers.get('x-oss-meta-sha256'), size: Number(response.headers.get('content-length')) };
  const hash = createHash('sha256');
  let size = 0;
  for await (const chunk of response.body) { hash.update(chunk); size += chunk.length; }
  return { sha256: hash.digest('hex'), size };
}

export function sameIdentity(a, b) {
  return a !== null && a.sha256 === b.sha256 && a.size === b.size;
}

// Shared blobs and version archives have the same immutable write contract.
// A concurrent creator may win the upload; a full read-back must still agree.
export async function publishImmutable(path, key, expected, dependencies = {}) {
  const archive = /^chentu\/releases\/([^/]+)\/([^/]+)$/.exec(key);
  let allowed = /^apeiron\/blobs\/sha256\/[a-f0-9]{64}$/.test(key);
  if (archive) {
    try {
      parseReleaseVersion(archive[1]);
      allowed = [`chentu-${archive[1]}.tar.gz`, `chentu-${archive[1]}.tar.gz.asc`, 'provenance.json'].includes(archive[2]);
    } catch { /* A version directory cannot be a traversal or floating channel. */ }
  }
  if (!allowed) throw new Error('Artifact key is outside the deployment publication prefixes');
  const identify = dependencies.fileIdentity || fileIdentity;
  const remote = dependencies.publicIdentity || publicIdentity;
  const run = dependencies.run || (async args => {
    const child = Bun.spawn(args, { stdout: 'ignore', stderr: 'inherit' });
    if (await child.exited !== 0) throw new Error('Immutable artifact upload failed');
  });
  const actual = await identify(path);
  if (!sameIdentity(actual, expected)) throw new Error('Artifact changed after package preparation');
  if (key.startsWith('apeiron/blobs/') && !key.endsWith('/' + actual.sha256))
    throw new Error('Content address differs from artifact bytes');
  const url = `https://downloads.apeironlab.cn/${key}`;
  // Existing objects passed a full read-back when created; do not download them again.
  const old = await remote(url, 'HEAD');
  if (old !== null) {
    if (!sameIdentity(old, actual)) throw new Error('Immutable artifact already exists with different bytes');
    return url;
  }
  try {
    if (actual.size > 5 * 1024 ** 3) {
      await run(['python3', fileURLToPath(new URL('../multipart-deployment-upload.py', import.meta.url)),
        '--file', path, '--key', key, '--sha256', actual.sha256, '--size', String(actual.size)]);
    } else {
      await run(['aliyun', 'ossutil', 'api', 'put-object', '--bucket', 'apeiron-bj-cli-downloads', '--key', key,
        '--body', `file://${path}`, '--forbid-overwrite', '--region', 'cn-beijing', '--object-acl', 'public-read',
        '--cache-control', 'public,max-age=31536000,immutable', '--metadata', `sha256=${actual.sha256}`]);
    }
  } catch (error) {
    // The server atomically rejects a concurrent writer. Only an identical
    // object already visible at the public URL makes this an idempotent retry.
    if (!sameIdentity(await remote(url), actual)) throw error;
    return url;
  }
  if (!sameIdentity(await remote(url), actual)) throw new Error('Public artifact read-back differs');
  return url;
}
