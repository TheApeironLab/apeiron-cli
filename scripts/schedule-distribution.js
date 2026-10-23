import { readFile } from 'node:fs/promises';
import { baseVersion } from './version.js';
import { compareReleaseVersions, parseReleaseVersion } from './lib/release-version.js';
const repo = 'TheApeironLab/apeiron-cli';
if (process.env.GITHUB_REPOSITORY !== repo || process.env.GITHUB_REF !== 'refs/heads/main')
  throw new Error('Unified nightly scheduling requires coordinator main');
function command(args, optional = false) {
  const p = Bun.spawnSync(args, { stdout: 'pipe', stderr: 'pipe' });
  if (p.exitCode) {
    if (optional && /HTTP 404/.test(p.stderr.toString())) return null;
    throw new Error(`${args[0]} ${args[1]} failed`);
  }
  return p.stdout.toString().trim();
}
const revision = command(['git', 'rev-parse', 'HEAD']);
if (revision !== process.env.GITHUB_SHA) throw new Error('Scheduler source mismatch');
function dispatchOriginal(tag) {
  const runs = JSON.parse(command(['gh', 'run', 'list', '--repo', repo, '--workflow', 'release.yml',
    '--branch', tag, '--commit', revision, '--limit', '1', '--json', 'databaseId']));
  if (runs.length) {
    console.log('This reviewed distribution is already scheduled; inspect or rerun its original workflow.');
  } else {
    command(['gh', 'workflow', 'run', 'release.yml', '--repo', repo, '--ref', tag]);
    console.log(`Dispatched the reviewed distribution ${tag}`);
  }
}
const stable = await fetch('https://downloads.apeironlab.cn/apeiron-cli/channels/stable.txt', { redirect: 'error', signal: AbortSignal.timeout(30000) });
if (stable.status !== 200 && stable.status !== 404) throw new Error('Cannot verify current stable channel');
if (stable.ok) {
  const version = (await stable.text()).trim();
  if (parseReleaseVersion(version).channel !== 'stable') throw new Error('Invalid stable pointer');
  if (compareReleaseVersions(baseVersion, version) <= 0) {
    console.log('Advance Cargo base before preparing the next development cycle.');
    process.exit(0);
  }
}
let input;
try { input = JSON.parse(await readFile('release/distribution-input.json', 'utf8')); }
catch (e) { if (e.code !== 'ENOENT') throw e; }
if (input && parseReleaseVersion(input.version).channel === 'nightly') {
  const tag = `v${input.version}`;
  if (parseReleaseVersion(input.version).base !== baseVersion) throw new Error('Distribution base differs from Cargo');
  const existing = command(['gh', 'api', `repos/${repo}/git/ref/tags/${tag}`], true);
  if (existing === null) {
    // Checks include public archive bytes/signature and exact committed pins.
    const check = Bun.spawnSync([process.execPath, 'scripts/check-distribution-source.js', 'release/distribution-input.json'],
      { stdout: 'inherit', stderr: 'inherit', env: { ...process.env, APEIRON_BUILD_VERSION: input.version } });
    if (check.exitCode) throw new Error('Merged distribution pins are not publishable');
    command(['gh', 'api', '--method', 'POST', `repos/${repo}/git/refs`, '-f', `ref=refs/tags/${tag}`, '-f', `sha=${revision}`]);
    dispatchOriginal(tag);
    process.exit(0);
  }
  const ref = JSON.parse(existing);
  if (ref.object.type !== 'commit') throw new Error('Unexpected nightly tag type');
  if (ref.object.sha === revision) {
    // Explicit retries belong to the original build/acceptance run. A new run
    // must not replace an immutable distribution receipt with a different ID.
    dispatchOriginal(tag);
    process.exit(0);
  }
  command(['git', 'merge-base', '--is-ancestor', ref.object.sha, revision]);
}
const pending = JSON.parse(command(['gh', 'pr', 'list', '--repo', repo, '--state', 'open', '--json', 'headRefName,url']));
if (pending.some(pr => pr.headRefName.startsWith('codex/distribution-'))) {
  console.log('A distribution pin PR is awaiting review; preserving that candidate.');
} else {
  command(['gh', 'workflow', 'run', 'prepare-distribution.yml', '--repo', repo, '--ref', 'main', '-f', 'channel=nightly']);
  console.log('Preparing a coordinated nightly candidate and pin PR; no channel changed.');
}
