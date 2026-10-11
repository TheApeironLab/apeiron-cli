import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { version } from './version';
import { sourceSnapshot, assertSourceUnchanged, inspectReleaseArtifact, releaseTargets } from './lib/release-provenance';
const out = process.argv[2];
if (!out || !isAbsolute(out))
    throw new Error('Usage: bun scripts/finalize-release.js /absolute/output');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const source = await sourceSnapshot(root);
const targets = Object.keys(releaseTargets);
const builds = {};
const files = {};
for (const name of [...targets.map(target => `apeiron-${version}-${target}.tar.gz`), 'install.sh']) {
    const bytes = await readFile(join(out, name));
    files[name] = { size: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
    if (name.endsWith('.tar.gz')) {
        const checksum = (await readFile(join(out, name + '.sha256'), 'utf8')).trim();
        if (checksum !== `${files[name].sha256}  ${name}`)
            throw new Error(`Build checksum mismatch: ${name}`);
        const target = targets.find(target => name === 'apeiron-' + version + '-' + target + '.tar.gz');
        const provenance = await inspectReleaseArtifact(join(out, name), source, version, target);
        builds[target] = provenance;
    }
}
if (files['install.sh'].sha256 !== createHash('sha256').update(await readFile(join(root, 'scripts/install.sh'))).digest('hex'))
    throw new Error('Install script differs from the release source commit');
const manifest = JSON.stringify({ schemaVersion: 2, version, revision: source.revision, source,
    toolchain: source.channel, targets, builds, files }, null, 2) + '\n';
assertSourceUnchanged(source, await sourceSnapshot(root));
await writeFile(join(out, 'release.json'), manifest);
const manifestHash = createHash('sha256').update(manifest).digest('hex');
await writeFile(join(out, 'SHA256SUMS'), Object.entries(files).map(([name, file]) => `${file.sha256}  ${name}\n`).join('') + `${manifestHash}  release.json\n`);
console.log(`Finalized ${version}: four platforms, install.sh, release.json, SHA256SUMS`);
