import { readFile, appendFile } from 'node:fs/promises';
import { baseVersion } from './version.js';
const recipe = JSON.parse(await readFile('release/recipe.json', 'utf8'));
if (recipe.schemaVersion !== 1 || recipe.baseVersion !== baseVersion) throw new Error('Release recipe/base mismatch');
if (process.env.GITHUB_REPOSITORY !== 'TheApeironLab/apeiron-cli' || process.env.GITHUB_REF !== 'refs/heads/main')
  throw new Error('Deployment preparation requires reviewed coordinator main');
const repositories = { apeiron: 'apeiron', chentu: 'chentu', systemSkills: 'apeiron-system-skills' };
for (const [name, repo] of Object.entries(repositories)) {
  const revision = recipe.sources[name];
  if (!/^[a-f0-9]{40}$/.test(revision || '')) throw new Error('Source requires a full commit');
  const child = Bun.spawnSync(['gh', 'api', `repos/TheApeironLab/${repo}/compare/${revision}...main`], { stdout: 'pipe', stderr: 'pipe' });
  if (child.exitCode) throw new Error(`Cannot verify merged source: ${repo}`);
  if (!['ahead', 'identical'].includes(JSON.parse(child.stdout.toString()).status)) throw new Error(`${repo} source is not on main`);
  await appendFile(process.env.GITHUB_OUTPUT, `${name}=${revision}\n`);
}
for (const profile of ['k3d-arm64', 'k3s-amd64']) {
  const p = recipe.platforms[profile];
  if (!p || !/^[a-f0-9]{64}$/.test(p.sha256) || !Number.isSafeInteger(p.size) || p.size < 1
      || p.url !== `https://downloads.apeironlab.cn/apeiron/blobs/sha256/${p.sha256}`)
    throw new Error('Missing reviewed platform seed: ' + profile);
}
