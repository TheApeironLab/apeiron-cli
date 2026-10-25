import { afterEach, expect, test } from 'bun:test';
import { copyFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const directories = [];
afterEach(async () => {
  await Promise.all(directories.splice(0).map(path => rm(path, { recursive: true, force: true })));
});

test('source checks exclude pinned upstream trees but still reject host TypeScript', async () => {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-source-check-'));
  directories.push(root);
  for (const directory of ['scripts', 'src', 'third_party/zygos', 'third_party/sudocode', 'third_party/other']) {
    await mkdir(join(root, directory), { recursive: true });
  }
  await copyFile(new URL('../scripts/check-sources.js', import.meta.url), join(root, 'scripts/check-sources.js'));
  await writeFile(join(root, 'third_party/zygos/prisma.config.ts'), 'export const enabled: boolean = true;');
  await writeFile(join(root, 'third_party/sudocode/sdk.ts'), 'export const enabled: boolean = true;');
  const check = () => Bun.spawnSync([process.execPath, 'scripts/check-sources.js'], { cwd: root });
  expect(check().exitCode).toBe(0);
  for (const file of ['src/invalid.ts', 'third_party/other/invalid.ts']) {
    await writeFile(join(root, file), 'export const invalid: boolean = true;');
    const result = check();
    expect(result.exitCode).not.toBe(0);
    expect(result.stderr.toString()).toContain(join(root, file));
    await rm(join(root, file));
  }
});
