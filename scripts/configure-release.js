import { appendFile, readFile } from 'node:fs/promises';
import { version as defaultVersion, baseVersion } from './version';
import { assertStableAlignment, parseReleaseVersion, resolveReleaseVersion } from './lib/release-version';

if (!process.env.GITHUB_ENV) throw new Error('This command configures a GitHub Actions build');
let version = defaultVersion;
if (!process.env.GITHUB_REF?.startsWith('refs/tags/') && !process.env.APEIRON_BUILD_VERSION) {
  try {
    const input = JSON.parse(await readFile('release/distribution-input.json', 'utf8'));
    version = resolveReleaseVersion(baseVersion, '', input.version);
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
}
const { channel } = parseReleaseVersion(version);
if (process.env.GITHUB_REF?.startsWith('refs/tags/') && channel === 'stable') {
  const apeiron = (await Bun.file(new URL('../third_party/apeiron/VERSION', import.meta.url)).text()).trim();
  const chentu = (await Bun.file(new URL('../release/distribution-input.json', import.meta.url)).json()).chentu.version;
  assertStableAlignment(version, apeiron, chentu);
}
await appendFile(process.env.GITHUB_ENV, `APEIRON_BUILD_VERSION=${version}\nCHENTU_BUILD_VERSION=${version}\n`);
console.log(`Building ${version} (${channel})`);
