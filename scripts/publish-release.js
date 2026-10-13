import { isAbsolute } from 'node:path';
import { publishRelease, tag, targetFromEnv } from './lib/publish-targets';
// Publication target comes from APEIRON_PUBLISH_TARGET (oss | forgejo | directory);
// see docs/releases.md. The public channel uses GitHub OIDC credentials scoped to
// this OSS prefix. Never use root keys in CI.
const out = process.argv[2];
const target = targetFromEnv(process.env);
if (!out || !isAbsolute(out))
    throw new Error(`Usage: bun scripts/publish-release.js /absolute/artifact/directory (APEIRON_PUBLISH_TARGET=${target})`);
const location = await publishRelease(out, target, process.env);
console.log(`Published ${tag} via ${target}: ${location}`);
