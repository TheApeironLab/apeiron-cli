import { readFile } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
import { publishImmutable } from './lib/release-files.js';

const path = process.argv[2];
if (!path || !isAbsolute(path)) throw new Error('An absolute prepared upload plan is required');
// CI upload authority lives on a separate environment-scoped RAM role. Local
// bootstrap uses the same immutable API with the operator's existing credentials.
if (process.env.GITHUB_ACTIONS && (process.env.GITHUB_REPOSITORY !== 'TheApeironLab/apeiron-cli'
    || process.env.GITHUB_REF !== 'refs/heads/main')) throw new Error('Deployment uploads require coordinator main');
const plan = JSON.parse(await readFile(path, 'utf8'));
if (!plan.objects || typeof plan.objects !== 'object' || Array.isArray(plan.objects)) throw new Error('Malformed upload plan');
const entries = Object.entries(plan.objects);
if (!entries.length) throw new Error('Empty upload plan');
// Blobs precede the versioned source archive: that archive must never point at
// resources that have not yet been uploaded and checked through the public URL.
for (const group of [entries.filter(([key]) => key.startsWith('apeiron/blobs/')),
  entries.filter(([key]) => !key.startsWith('apeiron/blobs/'))]) {
  for (let offset = 0; offset < group.length; offset += 4) {
    const results = await Promise.allSettled(group.slice(offset, offset + 4).map(async ([key, file]) => {
      if (!isAbsolute(file.path)) throw new Error('Upload artifact path must be absolute');
      await publishImmutable(file.path, key, file);
      console.log(`Verified ${key}`);
    }));
    const failed = results.find(result => result.status === 'rejected');
    if (failed) throw failed.reason;
  }
}
