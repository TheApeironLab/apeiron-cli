import { appendFile } from 'node:fs/promises';
import { version, channel } from './version';
import { assertStableAlignment } from './lib/release-version';

if (!process.env.GITHUB_ENV) throw new Error('This command configures a GitHub Actions build');
if (process.env.GITHUB_REF?.startsWith('refs/tags/') && channel === 'stable') {
  const apeiron = (await Bun.file(new URL('../third_party/apeiron/VERSION', import.meta.url)).text()).trim();
  const resources = await Bun.file(new URL('../runtime/src/app/resources.rs', import.meta.url)).text();
  const chentu = resources.match(/^const VERSION: &str = "([^"]+)";/m)?.[1];
  assertStableAlignment(version, apeiron, chentu);
}
await appendFile(process.env.GITHUB_ENV, `APEIRON_BUILD_VERSION=${version}\n`);
console.log(`Building ${version} (${channel})`);
