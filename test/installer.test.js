import { test } from 'bun:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink, lstat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
const installer = new URL('../scripts/install.sh', import.meta.url).pathname;
async function fixture(fn) {
    const directory = await mkdtemp(join(tmpdir(), 'apeiron-installer-test-'));
    try {
        for (const name of ['commands', 'source', 'home'])
            await mkdir(join(directory, name));
        const commands = join(directory, 'commands');
        const binary = join(directory, 'home/.local/bin/apeiron');
        const executable = async (name, text) => writeFile(join(commands, name), `#!/bin/sh\n${text}\n`, { mode: 0o755 });
        await executable('uname', 'if [ "$1" = -s ]; then echo Linux; else echo aarch64; fi');
        // Simulate an HTTPS download while retaining the real archive/checksum/install operations.
        await executable('curl', 'while [ "$#" -gt 0 ]; do case "$1" in --output) output=$2; shift 2;; https://*) url=$1; shift;; *) shift;; esac; done\nprintf "%s\\n" "$url" >> "$FIXTURE/requests"\ncp "$FIXTURE/${url##*/}" "$output"');
        const archive = join(directory, 'apeiron-1.2.3-linux-arm64.tar.gz');
        const checksums = join(directory, 'SHA256SUMS');
        await writeFile(join(directory, 'source/apeiron'), '#!/bin/sh\necho 1.2.3\n', { mode: 0o755 });
        assert.equal(spawnSync('tar', ['-czf', archive, '-C', join(directory, 'source'), 'apeiron']).status, 0);
        const digest = createHash('sha256').update(await readFile(archive)).digest('hex');
        await writeFile(checksums, `${digest}  apeiron-1.2.3-linux-arm64.tar.gz\n`);
        await writeFile(join(directory, 'latest.txt'), '1.2.3\n');
        await writeFile(join(directory, 'stable.txt'), '1.2.3\n');
        const environment = {
            PATH: `${commands}:${directory}/home/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin`, HOME: join(directory, 'home'), SHELL: '/bin/zsh', FIXTURE: directory,
        };
        const run = (...args) => spawnSync('/bin/sh', [installer, ...args], { encoding: 'utf8', env: environment });
        const shell = (script, env = {}) => spawnSync('/bin/sh', ['-c', script, 'installer-test', installer], { encoding: 'utf8', env: { ...environment, ...env } });
        const addVersion = async version => {
            await writeFile(join(directory, 'source/apeiron'), `#!/bin/sh\necho ${version}\n`, { mode: 0o755 });
            const path = join(directory, `apeiron-${version}-linux-arm64.tar.gz`);
            assert.equal(spawnSync('tar', ['-czf', path, '-C', join(directory, 'source'), 'apeiron']).status, 0);
            const sha = createHash('sha256').update(await readFile(path)).digest('hex');
            await writeFile(checksums, `${sha}  apeiron-${version}-linux-arm64.tar.gz\n`);
        };
        await fn({ directory, run, shell, archive, checksums, binary, addVersion });
    }
    finally {
        await rm(directory, { recursive: true, force: true });
    }
}
test('default installation follows stable even when latest or nightly points elsewhere', () => fixture(async ({ directory, run, binary }) => {
    await writeFile(join(directory, 'latest.txt'), '9.9.9-nightly.20261022.1\n');
    await writeFile(join(directory, 'nightly.txt'), '9.9.9-nightly.20261022.1\n');
    assert.equal(run().status, 0);
    assert.equal(spawnSync(binary, ['--version'], { encoding: 'utf8' }).stdout.trim(), '1.2.3');
    const requests = await readFile(join(directory, 'requests'), 'utf8');
    assert.match(requests, /\/channels\/stable\.txt/);
    assert.doesNotMatch(requests, /latest\.txt|nightly\.txt/);
}));
test('nightly is explicit and fixed versions bypass channel discovery', () => fixture(async ({ directory, run, binary, addVersion }) => {
    const version = '1.3.0-nightly.20261022.12';
    await addVersion(version);
    await writeFile(join(directory, 'nightly.txt'), `${version}\n`);
    const nightly = run('--channel', 'nightly');
    assert.equal(nightly.status, 0, nightly.stderr);
    assert.equal(spawnSync(binary, ['--version'], { encoding: 'utf8' }).stdout.trim(), version);
    assert.match(await readFile(join(directory, 'requests'), 'utf8'), /\/channels\/nightly\.txt/);
    await writeFile(join(directory, 'requests'), '');
    await rm(join(directory, 'nightly.txt'));
    assert.equal(run('--version', version).status, 0);
    assert.doesNotMatch(await readFile(join(directory, 'requests'), 'utf8'), /channels|latest\.txt/);
}));
test('missing or crossed channel pointers preserve the installation and never fall back', () => fixture(async ({ directory, run, binary }) => {
    assert.equal(run().status, 0);
    const original = await readFile(binary);
    await rm(join(directory, 'stable.txt'));
    assert.notEqual(run().status, 0);
    assert.notEqual(run('--channel', 'nightly').status, 0);
    await writeFile(join(directory, 'stable.txt'), '1.3.0-nightly.20261022.1\n');
    assert.notEqual(run().status, 0);
    await writeFile(join(directory, 'nightly.txt'), '1.2.3\n');
    assert.notEqual(run('--channel', 'nightly').status, 0);
    assert.deepEqual(await readFile(binary), original);
    assert.doesNotMatch(await readFile(join(directory, 'requests'), 'utf8'), /latest\.txt/);
}));
test('installer rejects conflicting and unknown channels before any download', () => fixture(async ({ directory, run, shell }) => {
    for (const args of [['--version', '1.2.3', '--channel', 'stable'], ['--channel', 'latest'], ['--channel', '../nightly'], ['--channel'], ['--version', '']]) {
        assert.equal(run(...args).status, 2);
    }
    assert.equal(shell('/bin/sh "$1" --version 1.2.3', { APEIRON_CHANNEL: 'nightly' }).status, 2);
    assert.equal(await Bun.file(join(directory, 'requests')).exists(), false);
}));
test('installer selects a standard bin already in PATH and works in the same parent shell', () => fixture(async ({ directory, shell, binary }) => {
    for (let i = 0; i < 2; i++) {
        const result = shell('/bin/sh "$1" && apeiron --version');
        assert.equal(result.status, 0, String(result.stderr));
        assert.match(String(result.stdout), /Ready in this terminal: apeiron init/);
        assert.match(String(result.stdout), /1\.2\.3\n$/);
    }
    assert.equal(spawnSync(binary, ['--version'], { encoding: 'utf8' }).stdout.trim(), '1.2.3');
    assert.equal(await Bun.file(join(directory, 'home/.zshrc')).exists(), false);
    assert.equal(await Bun.file(join(directory, 'commands/apeiron')).exists(), false);
}));
test('installer updates an existing command in place without changing its symlink target or shell hash', () => fixture(async ({ directory, shell, binary }) => {
    const bin = join(directory, 'home/.bun/bin');
    await mkdir(bin, { recursive: true });
    const source = join(directory, 'original-command');
    await writeFile(source, '#!/bin/sh\necho old-version\n', { mode: 0o755 });
    await symlink(source, join(bin, 'apeiron'));
    const result = shell('apeiron --version; /bin/sh "$1" && apeiron --version', {
        PATH: `${directory}/commands:${directory}/home/.local/bin:${bin}:/usr/bin:/bin`,
    });
    assert.equal(result.status, 0, String(result.stderr));
    assert.match(String(result.stdout), /^old-version\n/);
    assert.match(String(result.stdout), /1\.2\.3\n$/);
    assert.equal((await lstat(join(bin, 'apeiron'))).isSymbolicLink(), false);
    assert.equal(await readFile(source, 'utf8'), '#!/bin/sh\necho old-version\n');
    assert.equal(await Bun.file(binary).exists(), false);
}));
test('installer refuses success when no supported bin exists in PATH', () => fixture(async ({ directory, shell, binary }) => {
    const result = shell('/bin/sh "$1"', { PATH: `${directory}/commands:/usr/bin:/bin` });
    assert.notEqual(result.status, 0);
    assert.match(String(result.stderr), /No supported bin directory/);
    assert.equal(await Bun.file(binary).exists(), false);
    assert.equal(await Bun.file(join(directory, 'home/.zshrc')).exists(), false);
}));
test('installer does not overwrite an unrelated command in a project or temporary PATH directory', () => fixture(async ({ directory, run, binary }) => {
    const command = join(directory, 'commands/apeiron');
    await writeFile(command, '#!/bin/sh\necho project-command\n', { mode: 0o755 });
    const result = run();
    assert.notEqual(result.status, 0);
    assert.match(String(result.stderr), /takes precedence/);
    assert.equal(await readFile(command, 'utf8'), '#!/bin/sh\necho project-command\n');
    assert.equal(await Bun.file(binary).exists(), false);
}));
test('installer rejects corrupt downloads without replacing an existing installation', () => fixture(async ({ run, archive, binary }) => {
    assert.equal(run().status, 0);
    await writeFile(binary, 'previous-installation');
    await writeFile(archive, 'corrupt archive');
    const result = run();
    assert.notEqual(result.status, 0);
    assert.match(String(result.stderr), /Checksum mismatch/);
    assert.equal(await readFile(binary, 'utf8'), 'previous-installation');
}));
test('installer rejects duplicate checksum records and invalid versions', () => fixture(async ({ run, checksums }) => {
    await writeFile(checksums, (await readFile(checksums, 'utf8')).repeat(2));
    const duplicate = run();
    assert.notEqual(duplicate.status, 0);
    assert.match(String(duplicate.stderr), /duplicated/);
    const invalid = run('--version', '../unexpected');
    assert.notEqual(invalid.status, 0);
    assert.match(String(invalid.stderr), /Invalid release version/);
}));
test('custom install directory works without modifying the shell profile', () => fixture(async ({ directory, run }) => {
    const custom = join(directory, 'custom directory');
    const result = run('--install-dir', custom, '--no-modify-path');
    assert.equal(result.status, 0, String(result.stderr));
    assert.equal(spawnSync(join(custom, 'apeiron'), ['--version'], { encoding: 'utf8' }).stdout.trim(), '1.2.3');
    assert.equal(await Bun.file(join(directory, 'home/.zshrc')).exists(), false);
    assert.doesNotMatch(String(result.stdout), /In a new terminal/);
}));
