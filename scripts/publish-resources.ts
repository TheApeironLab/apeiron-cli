import { mkdir, writeFile } from 'node:fs/promises';
import { isAbsolute, join, relative } from 'node:path';
import { publicationPlan, verifyPublicBlob } from './lib/resource-publication';

const [bundle, output, mode] = process.argv.slice(2);
if (!bundle || !output || !isAbsolute(bundle) || !isAbsolute(output) || !['--plan', '--publish'].includes(mode ?? '') || !relative(bundle, output).startsWith('..')) {
  throw new Error('Usage: bun scripts/publish-resources.ts <absolute-bundle> <absolute-output-outside-bundle> --plan|--publish');
}
const plan = await publicationPlan(bundle);
await mkdir(output, { recursive: true });
await writeFile(join(output, 'upload-plan.json'), JSON.stringify(plan.blobs, null, 2) + '\n');
console.log(`Validated ${plan.catalog.files.length} files; ${plan.blobs.length} unique blobs; ${plan.blobs.reduce((sum, blob) => sum + blob.size, 0)} bytes`);
if (mode === '--publish') {
  for (const [index, blob] of plan.blobs.entries()) {
    console.log(`${index + 1}/${plan.blobs.length}: ${blob.key}`);
    if (await verifyPublicBlob(blob)) continue;
    const child = Bun.spawn(['aliyun', 'ossutil', 'cp', blob.source, `oss://apeiron-bj-cli-downloads/${blob.key}`,
      '--region', 'cn-beijing', '--acl', 'public-read', '--ignore-existing', '--no-progress',
      '--cache-control', 'public,max-age=31536000,immutable', '--metadata', `sha256=${blob.sha256}`], { stdout: 'inherit', stderr: 'inherit' });
    if (await child.exited !== 0 || !await verifyPublicBlob(blob)) throw new Error(`Upload verification failed: ${blob.key}`);
  }
}
// A plan is not a published catalog. Only emit the usable catalog after every
// object was retrieved and verified through the same public URL installers use.
await writeFile(join(output, mode === '--publish' ? 'install.json' : 'install.plan.json'), JSON.stringify(plan.catalog, null, 2) + '\n');
console.log(mode === '--publish' ? 'Published and verified resources; install.json is ready for a NEW deployment package.' : 'Plan only; no remote writes.');
