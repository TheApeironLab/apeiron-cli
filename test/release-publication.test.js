import { test, expect } from 'bun:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { baseVersion } from '../scripts/version';

const publisher = new URL('../scripts/publish-release.js', import.meta.url).pathname;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

// Run the real publisher against a filesystem-backed OSS/GitHub transport. This
// exercises ordering, checksums and retries without granting cloud write access.
async function fixture(version, fn) {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-publication-'));
  const out = join(root, 'out'), cloud = join(root, 'cloud'), bin = join(root, 'bin');
  try {
    for (const path of [out, cloud, bin]) await mkdir(path);
    const git = (...args) => {
      const result = Bun.spawnSync(['git', '-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', ...args], { cwd: root });
      if (result.exitCode) throw new Error(result.stderr.toString());
      return result.stdout.toString().trim();
    };
    git('init', '-q'); git('commit', '--allow-empty', '-qm', 'source'); git('tag', `v${version}`);
    const revision = git('rev-parse', 'HEAD');
    const files = {};
    for (const target of ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64']) {
      files[`apeiron-${version}-${target}.tar.gz`] = `archive for ${version} ${target}`;
    }
    files['install.sh'] = `installer for ${version}`;
    files['release.json'] = JSON.stringify({ schemaVersion: 2, version, baseVersion, revision,
      releaseChannel: version.includes('-nightly.') ? 'nightly' : 'stable' });
    for (const [name, bytes] of Object.entries(files)) await writeFile(join(out, name), bytes);
    await writeFile(join(out, 'SHA256SUMS'), Object.entries(files).map(([name, bytes]) => `${digest(bytes)}  ${name}\n`).join(''));
    const preload = join(root, 'transport.js');
    await writeFile(preload, `
      import { readFile } from 'node:fs/promises';
      import { join } from 'node:path';
      globalThis.fetch = async input => {
        const url = new URL(input);
        if (url.origin !== 'https://downloads.apeironlab.cn' || !url.pathname.startsWith('/apeiron-cli/')) throw new Error('Unexpected URL');
        try { return new Response(await readFile(join(process.env.FAKE_CLOUD, url.pathname.slice('/apeiron-cli/'.length)))); }
        catch (error) { if (error.code === 'ENOENT') return new Response('', {status:404}); throw error; }
      };
    `);
    const mock = `#!${process.execPath}
      import { readFile, writeFile, appendFile, mkdir, copyFile } from 'node:fs/promises';
      import { join, dirname, basename } from 'node:path';
      import { createHash } from 'node:crypto';
      const args = process.argv.slice(2), tool = basename(process.argv[1]);
      await appendFile(process.env.FAKE_LOG, JSON.stringify([tool, ...args]) + '\\n');
      if (tool === 'aliyun') {
        const key = args[3].replace('oss://apeiron-bj-cli-downloads/apeiron-cli/', '');
        if (key === process.env.FAIL_KEY) process.exit(1);
        const target = join(process.env.FAKE_CLOUD, key);
        await mkdir(dirname(target), {recursive:true});
        if (!(args.includes('--ignore-existing') && await Bun.file(target).exists())) await copyFile(args[2], target);
        if (key === process.env.CORRUPT_KEY) await writeFile(target, 'corrupted upload');
      } else {
        const stateFile = join(process.env.FAKE_CLOUD, 'github.json');
        const state = await Bun.file(stateFile).exists() ? JSON.parse(await readFile(stateFile, 'utf8')) : {exists:false, assets:[]};
        if (args[0] === 'release' && args[1] === 'view') {
          if (!state.exists) process.exit(1);
          console.log(args.includes('apiUrl') ? 'https://api.github.com/repos/TheApeironLab/apeiron-cli/releases/1' : '{}');
        } else if (args[0] === 'api') console.log(JSON.stringify(state.assets));
        else if (args[1] === 'create') state.exists = true;
        else if (args[1] === 'upload') {
          const bytes = await readFile(args[3]);
          state.assets.push({name:basename(args[3]), digest:'sha256:' + createHash('sha256').update(bytes).digest('hex')});
        } else if (args[1] !== 'edit') throw new Error('Unexpected gh command');
        await writeFile(stateFile, JSON.stringify(state));
      }
    `;
    for (const command of ['gh', 'aliyun']) await writeFile(join(bin, command), mock, { mode: 0o755 });
    const put = async (key, value) => { const path = join(cloud, key); await mkdir(dirname(path), { recursive: true }); await writeFile(path, value); };
    const get = key => readFile(join(cloud, key), 'utf8');
    const run = async (extra = {}) => {
      const child = Bun.spawn([process.execPath, '--preload', preload, publisher, out], {
        cwd: root, stdout: 'pipe', stderr: 'pipe',
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, GITHUB_REF: `refs/tags/v${version}`, APEIRON_BUILD_VERSION: '',
          FAKE_CLOUD: cloud, FAKE_LOG: join(root, 'commands.jsonl'), ...extra },
      });
      const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
      return { code, stdout, stderr };
    };
    const log = async () => (await readFile(join(root, 'commands.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
    await fn({ out, cloud, put, get, run, log, files });
  } finally { await rm(root, { recursive: true, force: true }); }
}

test('nightly publishes verified immutable files and only advances its own pointer', () => {
  const version = `${baseVersion}-nightly.20261022.42`;
  return fixture(version, async ({ put, get, run, log }) => {
    await put('latest.txt', 'old stable'); await put('channels/stable.txt', 'old stable'); await put('install.sh', 'old installer');
    const result = await run();
    expect(result.code, result.stderr).toBe(0);
    expect(await get('channels/nightly.txt')).toBe(`${version}\n`);
    expect(await get('latest.txt')).toBe('old stable');
    expect(await get('channels/stable.txt')).toBe('old stable');
    expect(await get('install.sh')).toBe('old installer');
    const commands = await log();
    expect(commands.find(args => args[1] === 'release' && args[2] === 'edit')).toContain('--latest=false');
    expect(commands.filter(args => args[0] === 'aliyun').at(-1)[4]).toEndWith('/channels/nightly.txt');
  });
});

test('stable publishes all aliases and a retry repairs an interrupted alias update', () => fixture(baseVersion, async ({ put, get, run }) => {
  await put('latest.txt', '0.1.0-rc.4\n'); await put('install.sh', 'old installer');
  expect((await run({ FAIL_KEY: 'install.sh' })).code).not.toBe(0);
  expect(await get('channels/stable.txt')).toBe(`${baseVersion}\n`);
  expect(await get('latest.txt')).toBe('0.1.0-rc.4\n');
  const retry = await run();
  expect(retry.code, retry.stderr).toBe(0);
  expect(await get('latest.txt')).toBe(`${baseVersion}\n`);
  expect(await get('install.sh')).toBe(`installer for ${baseVersion}`);
  expect((await run()).code).toBe(0);
}));

test('late older publication cannot roll back stable or replace the public installer', () => fixture(baseVersion, async ({ put, get, run, log }) => {
  await put('channels/stable.txt', '99.0.0\n'); await put('latest.txt', '99.0.0\n'); await put('install.sh', 'newer installer');
  const result = await run();
  expect(result.code, result.stderr).toBe(0);
  expect(await get('channels/stable.txt')).toBe('99.0.0\n');
  expect(await get('latest.txt')).toBe('99.0.0\n');
  expect(await get('install.sh')).toBe('newer installer');
  expect((await log()).find(args => args[1] === 'release' && args[2] === 'edit')).toContain('--latest=false');
}));

test('failed public checksum verification cannot promote a release', () => fixture(baseVersion, async ({ put, get, run, cloud }) => {
  await put('latest.txt', 'old'); await put('install.sh', 'old installer');
  const result = await run({ CORRUPT_KEY: `releases/${baseVersion}/apeiron-${baseVersion}-linux-arm64.tar.gz` });
  expect(result.code).not.toBe(0);
  expect(result.stderr).toContain('Public checksum mismatch');
  expect(await get('latest.txt')).toBe('old');
  expect(await get('install.sh')).toBe('old installer');
  expect(await Bun.file(join(cloud, 'channels/stable.txt')).exists()).toBe(false);
}));

test('different bytes under an existing version are never overwritten', () => fixture(baseVersion, async ({ put, get, run }) => {
  const key = `releases/${baseVersion}/apeiron-${baseVersion}-darwin-arm64.tar.gz`;
  await put(key, 'previously published bytes');
  const result = await run();
  expect(result.code).not.toBe(0);
  expect(result.stderr).toContain('Immutable release object differs');
  expect(await get(key)).toBe('previously published bytes');
}));
