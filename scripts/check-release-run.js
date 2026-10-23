import { version } from './version.js';

if (process.env.GITHUB_REPOSITORY !== 'TheApeironLab/apeiron-cli' || process.env.GITHUB_REF !== `refs/tags/v${version}`
    || !/^[1-9][0-9]*$/.test(process.env.BUILD_RUN_ID || ''))
  throw new Error('Unified publication requires this repository, a version tag and an exact build run ID');
// The last job of the build dispatches this coordinator. Its run may need a few
// seconds to transition to completed after the dispatch API returns.
let run;
for (let attempt = 0; attempt < 24; attempt++) {
  const result = Bun.spawnSync(['gh', 'api', `repos/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.BUILD_RUN_ID}`],
    { stdout: 'pipe', stderr: 'pipe' });
  if (result.exitCode !== 0) throw new Error('Cannot verify the candidate build run');
  run = JSON.parse(result.stdout.toString());
  if (run.status === 'completed') break;
  await new Promise(resolve => setTimeout(resolve, 5000));
}
if (run.conclusion !== 'success' || run.head_sha !== process.env.GITHUB_SHA || run.path !== '.github/workflows/release.yml'
    || !['push', 'workflow_dispatch'].includes(run.event) || run.repository?.full_name !== process.env.GITHUB_REPOSITORY)
  throw new Error('Candidate must be a successful release build at this exact source commit, not a PR build');
console.log(`Verified build ${process.env.BUILD_RUN_ID} at ${run.head_sha}`);
