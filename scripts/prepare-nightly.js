import { baseVersion } from './version';
import { parseReleaseVersion } from './lib/release-version';
import { planNightly } from './lib/nightly-release';

const repo = 'TheApeironLab/apeiron-cli';
const base = 'https://downloads.apeironlab.cn/apeiron-cli';
if (process.env.GITHUB_REPOSITORY !== repo || process.env.GITHUB_REF !== 'refs/heads/main')
  throw new Error('Nightly scheduling is restricted to this repository main branch');
function command(args, allowNotFound = false) {
  const result = Bun.spawnSync(args, { stdout: 'pipe', stderr: 'pipe' });
  if (result.exitCode !== 0) {
    if (allowNotFound && /HTTP 404/.test(result.stderr.toString())) return null;
    throw new Error(`${args[0]} ${args[1]} failed: ${result.stderr.toString()}`);
  }
  return result.stdout.toString().trim();
}
async function textAt(path, optional = false) {
  const response = await fetch(`${base}/${path}`, { redirect: 'error', signal: AbortSignal.timeout(30_000) });
  if (optional && response.status === 404) return null;
  if (response.status !== 200) throw new Error(`Cannot read ${path}: HTTP ${response.status}`);
  return (await response.text()).trim();
}
const stable = await textAt('channels/stable.txt', true);
const nightly = await textAt('channels/nightly.txt', true);
let previous = null;
if (nightly !== null) {
  if (parseReleaseVersion(nightly).channel !== 'nightly') throw new Error('Invalid nightly pointer');
  previous = JSON.parse(await textAt(`releases/${nightly}/release.json`));
  if (previous.version !== nightly) throw new Error('Nightly pointer/manifest mismatch');
}
const revision = command(['git', 'rev-parse', 'HEAD']);
if (revision !== process.env.GITHUB_SHA) throw new Error('Nightly checkout differs from the scheduled source commit');
const date = new Intl.DateTimeFormat('en-CA', { timeZone: 'Asia/Shanghai', year: 'numeric', month: '2-digit', day: '2-digit' })
  .format(new Date()).replaceAll('-', '');
const plan = planNightly({ base: baseVersion, revision, stable, previous, date, sequence: process.env.GITHUB_RUN_ID });
if (plan.skip) {
  console.log(plan.skip);
} else {
  const existing = command(['gh', 'api', `repos/${repo}/git/ref/tags/${plan.tag}`], true);
  if (existing !== null) {
    const ref = JSON.parse(existing);
    if (ref.object.type !== 'commit' || ref.object.sha !== revision) throw new Error('Nightly tag already points to another source');
  } else {
    command(['gh', 'api', '--method', 'POST', `repos/${repo}/git/refs`, '-f', `ref=refs/tags/${plan.tag}`, '-f', `sha=${revision}`]);
  }
  // GITHUB_TOKEN pushes do not trigger tag workflows; dispatch explicitly at the
  // tag so the existing tag-scoped OSS OIDC trust remains sufficient.
  command(['gh', 'workflow', 'run', 'release.yml', '--repo', repo, '--ref', plan.tag]);
  console.log(`Dispatched ${plan.tag} at ${revision}`);
}
