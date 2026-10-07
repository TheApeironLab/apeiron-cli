import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { version } from './version';
const out = process.argv[2];
if (!out || !isAbsolute(out))
    throw new Error('Usage: bun scripts/finalize-release.js /absolute/output');
const targets = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64'];
const files = {};
for (const name of [...targets.map(target => `apeiron-${version}-${target}.tar.gz`), 'install.sh']) {
    const bytes = await readFile(join(out, name));
    files[name] = { size: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
    if (name.endsWith('.tar.gz')) {
        const checksum = (await readFile(join(out, name + '.sha256'), 'utf8')).trim();
        if (checksum !== `${files[name].sha256}  ${name}`)
            throw new Error(`Build checksum mismatch: ${name}`);
    }
}
const revision = Bun.spawnSync(['git', 'rev-parse', 'HEAD']);
if (revision.exitCode !== 0)
    throw new Error('Cannot determine source revision');
const manifest = JSON.stringify({ schemaVersion: 1, version, revision: revision.stdout.toString().trim(), toolchain: (await Bun.file(new URL('../rust-toolchain.toml', import.meta.url)).text()).match(/channel = "([^"]+)"/)?.[1], targets, files }, null, 2) + '\n';
await writeFile(join(out, 'release.json'), manifest);
const manifestHash = createHash('sha256').update(manifest).digest('hex');
await writeFile(join(out, 'SHA256SUMS'), Object.entries(files).map(([name, file]) => `${file.sha256}  ${name}\n`).join('') + `${manifestHash}  release.json\n`);
console.log(`Finalized ${version}: four platforms, install.sh, release.json, SHA256SUMS`);
