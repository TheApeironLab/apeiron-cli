import { mkdir, writeFile, appendFile } from 'node:fs/promises';
import { join } from 'node:path';
import { baseVersion } from './version';
import { nightlyVersion } from './lib/release-version';

const { GITHUB_REPOSITORY, GITHUB_SHA, GITHUB_RUN_ID, GITHUB_OUTPUT, APEIRON_REVISION, CHENTU_REVISION } = process.env;
if (GITHUB_REPOSITORY !== 'TheApeironLab/apeiron-cli' || !GITHUB_OUTPUT)
  throw new Error('Candidate identity requires the coordinator workflow');
for (const sha of [GITHUB_SHA, APEIRON_REVISION, CHENTU_REVISION])
  if (!/^[a-f0-9]{40}$/.test(sha || '')) throw new Error('Candidate sources require full commits');
// A rerun after midnight keeps the original run's identity.
const created = new Date(process.env.GITHUB_RUN_CREATED_AT);
if (!Number.isFinite(created.getTime())) throw new Error('Candidate requires the original workflow creation time');
const date = new Intl.DateTimeFormat('en-CA', { timeZone: 'Asia/Shanghai', year: 'numeric', month: '2-digit', day: '2-digit' })
  .format(created).replaceAll('-', '');
const version = nightlyVersion(baseVersion, date, GITHUB_RUN_ID);
const output = process.argv[2];
if (!output) throw new Error('Usage: prepare-candidate.js OUTPUT');
await mkdir(output, { recursive: true });
await writeFile(join(output, 'candidate.json'), JSON.stringify({ schemaVersion: 1, version,
  status: 'candidate', acceptanceComplete: false, runId: GITHUB_RUN_ID,
  sources: { coordinator: GITHUB_SHA, apeiron: APEIRON_REVISION, chentu: CHENTU_REVISION } }, null, 2) + '\n');
await appendFile(GITHUB_OUTPUT, `version=${version}\n`);
console.log(`Building candidate ${version}; publication requires the separate acceptance gate.`);
