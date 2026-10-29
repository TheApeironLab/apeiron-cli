import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { publicDistribution, validateDistribution } from './lib/distribution.js';
import { verifyPublicDeployment } from './lib/deployment-signature.js';
import { baseVersion } from './version.js';
import { parseReleaseVersion } from './lib/release-version.js';

const repo = 'TheApeironLab/apeiron-cli';
const [inputPath] = process.argv.slice(2);
if (!inputPath || process.env.GITHUB_REPOSITORY !== repo || process.env.GITHUB_REF !== 'refs/heads/main')
  throw new Error('Pin preparation runs only from the coordinator main branch');
function run(args) {
  const p = Bun.spawnSync(args, { stdout: 'pipe', stderr: 'pipe' });
  if (p.exitCode) throw new Error(`${args[0]} ${args[1]} failed`);
  return p.stdout.toString().trim();
}
const source = run(['git', 'rev-parse', 'HEAD']);
if (source !== process.env.GITHUB_SHA || run(['git', 'status', '--porcelain'])) throw new Error('Pin preparation requires an unchanged clean checkout');
const input = JSON.parse(await readFile(inputPath, 'utf8'));
validateDistribution(input, { version: input.version, revision: source });
if (parseReleaseVersion(input.version).base !== baseVersion) throw new Error('Distribution must use the current Cargo base');
// Validate ancestry against the actual upstream main; a SHA-shaped value alone
// is not evidence that the application source was reviewed and merged.
if (!process.env.READ_TOKEN) throw new Error('An upstream read credential is required');
const header = 'AUTHORIZATION: basic ' + Buffer.from('x-access-token:' + process.env.READ_TOKEN).toString('base64');
run(['git', '-c', 'http.https://github.com/.extraheader=', '-c', `http.https://github.com/.extraheader=${header}`, '-C', 'third_party/apeiron', 'fetch', 'origin', 'main']);
run(['git', '-C', 'third_party/apeiron', 'merge-base', '--is-ancestor', input.apeiron.revision, 'origin/main']);
await verifyPublicDeployment(input.chentu);
const branch = `codex/distribution-${input.version}`;
const existing = JSON.parse(run(['gh', 'pr', 'list', '--repo', repo, '--state', 'all', '--head', branch, '--json', 'number,state,url']));
if (existing.length) throw new Error(`Distribution already has a review: ${existing[0].url}; resume that immutable candidate`);
run(['git', 'switch', '-c', branch]);
run(['git', '-C', 'third_party/apeiron', 'checkout', '--detach', input.apeiron.revision]);
await mkdir('release', { recursive: true });
await writeFile('release/distribution-input.json', JSON.stringify(publicDistribution(input), null, 2) + '\n');
run(['git', 'add', 'third_party/apeiron', 'release/distribution-input.json']);
run(['git', 'diff', '--cached', '--check']);
run(['git', '-c', 'user.name=github-actions[bot]', '-c', 'user.email=41898282+github-actions[bot]@users.noreply.github.com',
  'commit', '-m', `chore(release): pin distribution ${input.version}`]);
run(['git', 'push', 'origin', `HEAD:refs/heads/${branch}`]);
const temporary = await mkdtemp(join(tmpdir(), 'distribution-pr-'));
try {
  const body = join(temporary, 'body.txt');
  await writeFile(body, `Bind CLI ${input.version} to the prepared Apeiron and Chentu artifacts.\n\nApeiron: ${input.apeiron.revision}\nChentu: ${input.chentu.revision}\nArchive SHA-256: ${input.chentu.sha256}\n\nAll upstream objects were read back and checked before this PR was created. This is a source pin review, not installation acceptance. The four-target public-download installation gate must pass before a download channel advances. Stable publication still requires an explicitly authorized stable tag.\n`);
  console.log(run(['gh', 'pr', 'create', '--repo', repo, '--base', 'main', '--head', branch,
    '--title', `chore(release): pin distribution ${input.version}`, '--body-file', body]));
  // The repository-scoped writer creates a normal pull_request event. That
  // workflow reads the committed distribution version and runs the full build.
} finally {
  await rm(temporary, { recursive: true, force: true });
}
