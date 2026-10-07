import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat } from 'node:fs/promises';
import { join } from 'node:path';
import { buildSupport } from './rust';
export const resourceOrigin = 'https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com';
export const blobPrefix = 'apeiron/blobs/sha256';
export async function digestFile(path) {
    const hash = createHash('sha256');
    for await (const chunk of createReadStream(path))
        hash.update(chunk);
    return hash.digest('hex');
}
// Validate the entire input before publishing any byte. Platform and component
// ownership stay in the catalog; identical bytes have one storage identity.
export async function publicationPlan(bundle) {
    if (!(await lstat(bundle)).isDirectory())
        throw new Error('Bundle must be a real directory');
    const catalog = await buildSupport('catalog', join(bundle, 'chentu'));
    const blobs = new Map();
    for (const file of catalog.files) {
        let source = bundle;
        for (const part of file.path.split('/')) {
            source = join(source, part);
            if ((await lstat(source)).isSymbolicLink())
                throw new Error(`Symlink is not publishable: ${file.path}`);
        }
        const stat = await lstat(source);
        if (!stat.isFile() || stat.size !== file.size || await digestFile(source) !== file.sha256)
            throw new Error(`Resource checksum mismatch: ${file.path}`);
        const key = `${blobPrefix}/${file.sha256}`;
        file.url = `${resourceOrigin}/${key}`;
        blobs.set(key, { source, key, size: file.size, sha256: file.sha256 });
    }
    return { catalog, blobs: [...blobs.values()] };
}
export async function verifyPublicBlob(blob) {
    const response = await fetch(`${resourceOrigin}/${blob.key}`, { redirect: 'error', signal: AbortSignal.timeout(30 * 60_000) });
    if (response.status === 404)
        return false;
    if (response.status !== 200 || !response.body)
        throw new Error(`Blob download failed: ${blob.key} HTTP ${response.status}`);
    const hash = createHash('sha256');
    let size = 0;
    const reader = response.body.getReader();
    while (true) {
        const { value: chunk, done } = await reader.read();
        if (done)
            break;
        size += chunk.byteLength;
        if (size > blob.size)
            throw new Error(`Oversized public blob: ${blob.key}`);
        hash.update(chunk);
    }
    if (size !== blob.size || hash.digest('hex') !== blob.sha256)
        throw new Error(`Immutable public blob differs: ${blob.key}`);
    return true;
}
