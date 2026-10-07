import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile, copyFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { version } from './version';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const targets = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64'];
const options = process.argv.slice(2);
const out = options[0];
const selected = [options[1] || `${process.platform}-${process.arch}`];
const triples = { 'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin', 'linux-arm64': 'aarch64-unknown-linux-musl', 'linux-x64': 'x86_64-unknown-linux-musl' };
if (!out || !isAbsolute(out) || options.length > 2 || selected.some(t => !targets.includes(t))) {
    throw new Error('Usage: bun run build:release /absolute/output [darwin-arm64|darwin-x64|linux-arm64|linux-x64]');
}
if (resolve(out) === root || resolve(out).startsWith(root + '/'))
    throw new Error('Release output must be outside the checkout');
await mkdir(out, { recursive: true });
const readme = `Apeiron CLI ${version}\n\nRun ./apeiron init to open setup in your default browser.\nNo Node.js or Bun installation is required for the setup wizard.\nFor a headless server, use ./apeiron init --no-open --port 3210 and\nssh -L 3210:127.0.0.1:3210 user@server; open the printed URL locally.\n\nDocker is required for local K3d testing. Native K3s needs a compatible\nUbuntu host and deployment tools. CLI platform support does not imply\na matching Chentu deployment package exists for every platform.\nFor offline deployment, transfer the full matching Chentu package too.\n\nOther application commands require their separately configured executables.\nThe macOS preview binaries are not Developer ID notarized.\n`;
for (const target of selected) {
    const name = `apeiron-${version}-${target}.tar.gz`;
    if (await Bun.file(join(out, name)).exists())
        throw new Error(`Refusing to replace ${name}`);
    const stage = await mkdtemp(join(tmpdir(), 'apeiron-release-'));
    try {
        const triple = triples[target];
        const build = Bun.spawn(['cargo', 'build', '--release', '--locked', '--target', triple], {
            cwd: root, stdout: 'inherit', stderr: 'inherit',
        });
        if (await build.exited !== 0)
            throw new Error(`Build failed for ${target}`);
        const metadata = Bun.spawnSync(['cargo', 'metadata', '--no-deps', '--format-version', '1'], { cwd: root });
        if (metadata.exitCode !== 0)
            throw new Error('Cannot locate Cargo output');
        const targetDirectory = JSON.parse(metadata.stdout.toString()).target_directory;
        await copyFile(join(targetDirectory, triple, 'release/apeiron'), join(stage, 'apeiron'));
        await writeFile(join(stage, 'README.md'), readme);
        const archive = Bun.spawn(['tar', '-czf', join(out, name), '-C', stage, 'apeiron', 'README.md'], { stdout: 'inherit', stderr: 'inherit' });
        if (await archive.exited !== 0)
            throw new Error(`Archive failed for ${target}`);
        const digest = createHash('sha256').update(await readFile(join(out, name))).digest('hex');
        await writeFile(join(out, `${name}.sha256`), `${digest}  ${name}\n`);
        console.log(`Built ${name}`);
    }
    finally {
        await rm(stage, { recursive: true, force: true });
    }
}
await copyFile(join(root, 'scripts/install.sh'), join(out, 'install.sh'));
