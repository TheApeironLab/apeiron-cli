import { test, expect } from 'bun:test';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

test('PR builds use the committed distribution version and reject a different Cargo base', async () => {
  const root = await mkdtemp(join(tmpdir(), 'configure-release-'));
  try {
    await mkdir(join(root, 'scripts/lib'), { recursive: true });
    await mkdir(join(root, 'release'));
    for (const name of ['configure-release.js', 'version.js', 'lib/release-version.js'])
      await cp(new URL('../scripts/' + name, import.meta.url), join(root, 'scripts', name));
    await writeFile(join(root, 'Cargo.toml'), '[workspace.package]\nversion="0.2.1"\n');
    const env = { ...process.env, GITHUB_ENV: join(root, 'env'), GITHUB_REF: 'refs/pull/40/merge', APEIRON_BUILD_VERSION: '' };
    async function build(version) {
      await writeFile(join(root, 'release/distribution-input.json'), JSON.stringify({ version }));
      await writeFile(env.GITHUB_ENV, '');
      return Bun.spawnSync([process.execPath, 'scripts/configure-release.js'], { cwd: root, env, stdout: 'pipe', stderr: 'pipe' });
    }
    const nightly = '0.2.1-nightly.20261023.123';
    expect((await build(nightly)).exitCode).toBe(0);
    expect(await readFile(env.GITHUB_ENV, 'utf8')).toBe(`APEIRON_BUILD_VERSION=${nightly}\nCHENTU_BUILD_VERSION=${nightly}\n`);
    expect((await build('0.2.2')).exitCode).not.toBe(0);
    expect(await readFile(env.GITHUB_ENV, 'utf8')).toBe('');
    expect((await build('0.2.1-rc.5')).exitCode).not.toBe(0);
  } finally { await rm(root, { recursive: true, force: true }); }
});
