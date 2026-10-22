import { appendFile } from 'node:fs/promises';
import { version, channel } from './version';

if (!process.env.GITHUB_ENV) throw new Error('This command configures a GitHub Actions build');
await appendFile(process.env.GITHUB_ENV, `APEIRON_BUILD_VERSION=${version}\n`);
console.log(`Building ${version} (${channel})`);
