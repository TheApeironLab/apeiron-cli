import { test, expect } from 'bun:test';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { baseVersion } from '../scripts/version';

const scheduler = new URL('../scripts/prepare-nightly.js', import.meta.url).pathname;
async function fixture(fn) {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-nightly-'));
  const cloud = join(root, 'cloud'), bin = join(root, 'bin');
  try {
    await mkdir(cloud); await mkdir(bin);
    const git = (...args) => {
      const result = Bun.spawnSync(['git', '-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', ...args], { cwd: root });
      if (result.exitCode) throw new Error(result.stderr.toString());
      return result.stdout.toString().trim();
    };
    git('init', '-q'); git('commit', '--allow-empty', '-qm', 'source');
    const revision = git('rev-parse', 'HEAD');
    const preload = join(root, 'transport.js');
    await writeFile(preload, `
      import { readFile } from 'node:fs/promises';
      import { join } from 'node:path';
      globalThis.fetch = async input => {
        const url = new URL(input);
        if (url.origin !== 'https://downloads.apeironlab.cn' || !url.pathname.startsWith('/apeiron-cli/')) throw new Error('Unexpected URL');
        try { return new Response(await readFile(join(process.env.FAKE_CLOUD, url.pathname.slice('/apeiron-cli/'.length)))); }
        catch (e) { if (e.code === 'ENOENT') return new Response('', {status:404}); throw e; }
      };
    `);
    await writeFile(join(bin, 'gh'), `#!${process.execPath}
      import { appendFile, writeFile, readFile } from 'node:fs/promises';
      const args = process.argv.slice(2), file = process.env.FAKE_TAG;
      await appendFile(process.env.FAKE_LOG, JSON.stringify(args)+'\\n');
      if (args[0] === 'workflow') {
        if (process.env.FAIL_DISPATCH === '1') process.exit(1);
      } else if (args.includes('POST')) {
        const tag = args.find(a => a.startsWith('ref=')).slice('ref=refs/tags/'.length);
        const sha = args.find(a => a.startsWith('sha=')).slice(4);
        await writeFile(file, JSON.stringify({tag,object:{type:'commit',sha}}));
      } else {
        if (!(await Bun.file(file).exists())) { console.error('HTTP 404'); process.exit(1); }
        console.log(await readFile(file,'utf8'));
      }
    `, { mode: 0o755 });
    const put = async (key, value) => { const path = join(cloud, key); await mkdir(dirname(path), { recursive: true }); await writeFile(path, value); };
    const run = async (extra = {}) => {
      const child = Bun.spawn([process.execPath, '--preload', preload, scheduler], {
        cwd: root, stdout: 'pipe', stderr: 'pipe', env: {
          ...process.env, PATH: `${bin}:${process.env.PATH}`, GITHUB_REF: 'refs/heads/main', GITHUB_REPOSITORY: 'TheApeironLab/apeiron-cli',
          GITHUB_RUN_ID: '123', GITHUB_SHA: revision, APEIRON_BUILD_VERSION: '', FAKE_CLOUD: cloud,
          FAKE_TAG: join(root, 'tag.json'), FAKE_LOG: join(root, 'commands.jsonl'), ...extra,
        },
      });
      const [code, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
      return { code, stdout, stderr };
    };
    const log = async () => await Bun.file(join(root, 'commands.jsonl')).exists()
      ? (await readFile(join(root, 'commands.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse) : [];
    await fn({ root, put, run, log, revision });
  } finally { await rm(root, { recursive: true, force: true }); }
}

test('nightly creates a fixed source tag and explicitly dispatches it; retry reuses the tag', () => fixture(async ({ root, run, log, revision }) => {
  expect((await run({ FAIL_DISPATCH: '1' })).code).not.toBe(0);
  const tag = JSON.parse(await readFile(join(root, 'tag.json'), 'utf8'));
  expect(tag.object.sha).toBe(revision);
  expect(tag.tag.startsWith(`v${baseVersion}-nightly.`)).toBe(true);
  expect(tag.tag.endsWith('.123')).toBe(true);
  const result = await run();
  expect(result.code, result.stderr).toBe(0);
  const commands = await log();
  expect(commands.filter(args => args.includes('POST'))).toHaveLength(1);
  expect(commands.at(-1)).toEqual(['workflow', 'run', 'release.yml', '--repo', 'TheApeironLab/apeiron-cli', '--ref', tag.tag]);
}));

test('successful unchanged source and already stable bases cause no tag or dispatch writes', () => fixture(async ({ put, run, log, revision }) => {
  const version = `${baseVersion}-nightly.20261021.1`;
  await put('channels/nightly.txt', `${version}\n`);
  await put(`releases/${version}/release.json`, JSON.stringify({ schemaVersion: 2, version, revision }));
  expect((await run()).stdout).toContain('already has');
  await put('channels/stable.txt', `${baseVersion}\n`);
  expect((await run()).stdout).toContain('already stable');
  expect(await log()).toEqual([]);
}));

test('scheduler rejects other branches, changed tag targets and untrusted channel paths', () => fixture(async ({ root, put, run, log }) => {
  expect((await run({ GITHUB_REF: 'refs/heads/feature' })).code).not.toBe(0);
  expect((await run({ GITHUB_REPOSITORY: 'someone/apeiron-cli' })).code).not.toBe(0);
  expect(await log()).toEqual([]);
  await writeFile(join(root, 'tag.json'), JSON.stringify({ object: { type: 'commit', sha: 'f'.repeat(40) } }));
  expect((await run()).stderr).toContain('already points to another source');
  expect((await log()).some(args => args[0] === 'workflow' || args.includes('POST'))).toBe(false);
  await put('channels/nightly.txt', '../unexpected');
  expect((await run()).code).not.toBe(0);
}));
