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
const resources = await readFile(new URL('../runtime/src/app/resources.rs', import.meta.url), 'utf8');
for (const [constant, expected] of [['VERSION', input.chentu.version], ['REVISION', input.chentu.revision], ['DIGEST', input.chentu.sha256]]) {
  const actual = resources.match(new RegExp(`^const ${constant}: &str = "([^"]+)";`, 'm'))?.[1];
  if (actual !== expected) throw new Error(`Distribution differs from compiled Chentu ${constant}`);
}
await verifyPublicDeployment(input.chentu);
console.log(`Validated source pins and public Chentu archive for ${version}`);
