import { afterEach, expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { publicationPlan, blobPrefix } from '../scripts/lib/resource-publication';
const dirs = [];
afterEach(async () => { await Promise.all(dirs.splice(0).map(path => rm(path, { recursive: true, force: true }))); });
async function fixture() {
    const root = await mkdtemp(join(tmpdir(), 'publication-'));
    dirs.push(root);
    await mkdir(join(root, 'chentu/setup'), { recursive: true });
    const sha256 = createHash('sha256').update('same bytes').digest('hex');
    const files = ['arm.tar', 'shared.tar'].map(path => ({ path, sha256, size: 10, url: `https://example.com/rc.2/${path}` }));
    await Promise.all(files.map(file => writeFile(join(root, file.path), 'same bytes')));
    await writeFile(join(root, 'chentu/setup/install.json'), JSON.stringify({ schemaVersion: 1, files, targets: {}, components: {} }));
    return root;
}
test('publication deduplicates bytes and preserves offline paths and source catalog', async () => {
    const root = await fixture();
    const before = await readFile(join(root, 'chentu/setup/install.json'), 'utf8');
    const plan = await publicationPlan(root);
    expect(plan.blobs).toHaveLength(1);
    expect(plan.catalog.files.map(file => file.path)).toEqual(['arm.tar', 'shared.tar']);
    expect(plan.catalog.files[0].url).toContain(`${blobPrefix}/${plan.blobs[0].sha256}`);
    expect(plan.catalog.files[1].url).toBe(plan.catalog.files[0].url);
    expect(await readFile(join(root, 'chentu/setup/install.json'), 'utf8')).toBe(before);
});
test('tampered input fails before a publication plan is returned', async () => {
    const root = await fixture();
    await writeFile(join(root, 'arm.tar'), 'evil bytes');
    await expect(publicationPlan(root)).rejects.toThrow('checksum mismatch');
});
test('resource symlinks cannot publish files outside the bundle', async () => {
    const root = await fixture();
    await rm(join(root, 'arm.tar'));
    await symlink(join(root, 'shared.tar'), join(root, 'arm.tar'));
    await expect(publicationPlan(root)).rejects.toThrow('Symlink');
});
test('public verification checks bytes rather than trusting object metadata', async () => {
    const { verifyPublicBlob } = await import('../scripts/lib/resource-publication');
    const original = globalThis.fetch;
    const blob = { source: '', key: 'test', size: 10, sha256: createHash('sha256').update('same bytes').digest('hex') };
    try {
        globalThis.fetch = (async () => new Response('same bytes'));
        expect(await verifyPublicBlob(blob)).toBe(true);
        globalThis.fetch = (async () => new Response('evil bytes'));
        await expect(verifyPublicBlob(blob)).rejects.toThrow('differs');
        globalThis.fetch = (async () => new Response('', { status: 404 }));
        expect(await verifyPublicBlob(blob)).toBe(false);
        globalThis.fetch = (async () => new Response('', { status: 403 }));
        await expect(verifyPublicBlob(blob)).rejects.toThrow('HTTP 403');
    }
    finally {
        globalThis.fetch = original;
    }
});
