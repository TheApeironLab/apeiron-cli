import { readFile } from 'node:fs/promises';
import { validateDistribution } from './lib/distribution.js';
import { version } from './version.js';
import { verifyPublicDeployment } from './lib/deployment-signature.js';

const inputPath = process.argv[2];
if (!inputPath) throw new Error('A committed distribution input is required');
const git = (...args) => {
  const r = Bun.spawnSync(['git', ...args], { stdout: 'pipe', stderr: 'pipe' });
  if (r.exitCode !== 0) throw new Error(`Cannot read release source: ${args[0]}`);
  return r.stdout.toString().trim();
};
const input = validateDistribution(JSON.parse(await readFile(inputPath, 'utf8')), { version, revision: git('rev-parse', 'HEAD') });
const apeiron = git('rev-parse', 'HEAD:third_party/apeiron');
if (input.apeiron.revision !== apeiron) throw new Error('Distribution does not match the Apeiron gitlink');
const compiled = JSON.parse(await readFile(new URL('../release/distribution-input.json', import.meta.url), 'utf8'));
for (const field of ['version', 'revision', 'sha256', 'url']) {
  if (compiled.chentu[field] !== input.chentu[field]) throw new Error(`Distribution differs from compiled Chentu ${field}`);
}
const engine = git('rev-parse', 'HEAD:third_party/chentu');
if (engine !== git('-C', 'third_party/chentu', 'rev-parse', 'HEAD')) throw new Error('Chentu engine differs from its gitlink');
await verifyPublicDeployment(input.chentu);
console.log(`Validated source pins and public Chentu archive for ${version}`);
