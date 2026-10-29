import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile, copyFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { version } from './version';
import { prepareSudocode } from './lib/sudocode-runtime.js';
import { sourceSnapshot, assertSourceUnchanged, buildProvenance, compilerVersion, releaseTargets } from './lib/release-provenance';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const targets = Object.keys(releaseTargets);
const options = process.argv.slice(2);
const out = options[0];
const selected = [options[1] || `${process.platform}-${process.arch}`];
const triples = releaseTargets;
if (!out || !isAbsolute(out) || options.length > 2 || selected.some(t => !targets.includes(t))) {
    throw new Error('Usage: bun run build:release /absolute/output [darwin-arm64|darwin-x64|linux-arm64|linux-x64]');
}
if (resolve(out) === root || resolve(out).startsWith(root + '/'))
    throw new Error('Release output must be outside the checkout');
const source = await sourceSnapshot(root);
const rustc = compilerVersion('rustc', root);
const cargo = compilerVersion('cargo', root);
await mkdir(out, { recursive: true });
const readme = `Apeiron CLI ${version}\n\nRun ./apeiron init to open setup in your default browser.\nNo Node.js or Bun installation is required for the setup wizard.\nFor a headless server, use ./apeiron init --no-open --port 3210 and\nssh -L 3210:127.0.0.1:3210 user@server; open the printed URL locally.\n\nDocker is required for local K3d testing. Native K3s needs a compatible\nUbuntu host and deployment tools. CLI platform support does not imply\na matching Chentu deployment package exists for every platform.\nFor offline deployment, transfer the full matching Chentu package too.\n\napeiron sudo starts the bundled Sudocode runtime without a Gateway or platform backend.\nRuntime version and digest are recorded in build-provenance.json.\nOther application commands require their separately configured executables.\nThe macOS preview binaries are not Developer ID notarized.\n`;
for (const target of selected) {
    const name = `apeiron-${version}-${target}.tar.gz`;
    if (await Bun.file(join(out, name)).exists())
        throw new Error(`Refusing to replace ${name}`);
    const stage = await mkdtemp(join(tmpdir(), 'apeiron-release-'));
    try {
        const triple = triples[target];
        const sudocode = await prepareSudocode(root, target, stage);
        const build = Bun.spawn(['cargo', 'build', '--release', '--locked', '--target', triple], {
            cwd: root, stdout: 'inherit', stderr: 'inherit',
            env: { ...process.env, APEIRON_BUILD_VERSION: version, CHENTU_BUILD_VERSION: version, APEIRON_RELEASE_SOURCE_REVISION: source.revision, APEIRON_SCODE_ARCHIVE: sudocode.path },
        });
        if (await build.exited !== 0)
            throw new Error(`Build failed for ${target}`);
        const metadata = Bun.spawnSync(['cargo', 'metadata', '--no-deps', '--format-version', '1'], { cwd: root });
        if (metadata.exitCode !== 0)
            throw new Error('Cannot locate Cargo output');
        const targetDirectory = JSON.parse(metadata.stdout.toString()).target_directory;
        await copyFile(join(targetDirectory, triple, 'release/apeiron'), join(stage, 'apeiron'));
        assertSourceUnchanged(source, await sourceSnapshot(root));
        const binary = await readFile(join(stage, 'apeiron'));
        const provenance = buildProvenance(source, version, target, binary, rustc, cargo);
        const { path: _payloadPath, ...runtimeProvenance } = sudocode;
        provenance.sudocode = runtimeProvenance;
        await writeFile(join(stage, 'build-provenance.json'), JSON.stringify(provenance, null, 2) + '\n');
        await writeFile(join(stage, 'README.md'), readme + '\nApeiron notices\n\n' + await readFile(join(root, 'runtime/NOTICE'), 'utf8') + '\nSudocode license\n\n' + await readFile(join(root, 'third_party/sudocode/LICENSE'), 'utf8'));
        const archive = Bun.spawn(['tar', '-czf', join(out, name), '-C', stage, 'apeiron', 'README.md', 'build-provenance.json'], { stdout: 'inherit', stderr: 'inherit' });
        if (await archive.exited !== 0)
            throw new Error(`Archive failed for ${target}`);
        const digest = createHash('sha256').update(await readFile(join(out, name))).digest('hex');
        await writeFile(join(out, `${name}.sha256`), `${digest}  ${name}\n`);
        const archiveBytes = await readFile(join(out, name));
        await writeFile(join(out, name + '.provenance.json'), JSON.stringify({ ...provenance, archive: { size: archiveBytes.length, sha256: digest } }, null, 2) + '\n');
        console.log(`Built ${name}`);
    }
    finally {
        await rm(stage, { recursive: true, force: true });
    }
}
assertSourceUnchanged(source, await sourceSnapshot(root));
await copyFile(join(root, 'scripts/install.sh'), join(out, 'install.sh'));
