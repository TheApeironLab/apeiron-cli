import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { acceptanceTargets, acceptanceChecks, validateDistribution, validateAcceptanceSet, publicDistribution } from './lib/distribution.js';

const [out, inputPath, receiptsPath] = process.argv.slice(2);
if (![out, inputPath, receiptsPath].every(p => p && isAbsolute(p)))
  throw new Error('Usage: bun scripts/finalize-distribution.js /release /distribution-input.json /receipts');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const release = JSON.parse(await readFile(join(out, 'release.json'), 'utf8'));
const inputBytes = await readFile(inputPath);
const input = validateDistribution(JSON.parse(inputBytes), release);
const receipts = await Promise.all(Object.keys(acceptanceTargets).map(async target =>
  JSON.parse(await readFile(join(receiptsPath, `${target}.json`), 'utf8'))));
validateAcceptanceSet(receipts, release, hash(inputBytes), process.env.GITHUB_RUN_ID);
const keys = ['schemaVersion', 'version', 'revision', 'target', 'distributionSha256', 'archiveSha256', 'runId',
  'os', 'arch', 'topology', 'osRelease', 'servers', 'agents'];
const distribution = { ...publicDistribution(input), cli: { revision: release.revision, files: release.files },
  inputSha256: hash(inputBytes), acceptance: receipts.map(r => ({ ...Object.fromEntries(keys.map(k => [k, r[k]])),
    checks: Object.fromEntries(acceptanceChecks.map(k => [k, r.checks[k]])) })) };
// This file is a publication artifact; private runner profiles and raw installation
// logs must never be copied into the public release directory.
const bytes = JSON.stringify(distribution, null, 2) + '\n';
await writeFile(join(out, 'distribution.json'), bytes, { flag: 'wx' });
const sumsPath = join(out, 'SHA256SUMS');
await writeFile(sumsPath, await readFile(sumsPath, 'utf8') + `${hash(bytes)}  distribution.json\n`);
console.log(`Validated ${release.version}: both K3d hosts, native single-node and three-server K3s`);
