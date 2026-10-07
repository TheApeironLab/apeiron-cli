import { expect, test } from 'bun:test';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { readInstallCatalog, installPlan } from '../src/resources/install';

test('release selects only the requested target despite identical archive paths', async () => {
  const root = await mkdtemp(join(tmpdir(), 'target-catalog-'));
  try {
    await mkdir(join(root, 'setup'));
    const catalogs = Object.fromEntries(['k3d-arm64', 'k3s-arm64'].map((target, index) => [target, {
      schemaVersion: 1,
      targets: { [target]: { base: ['app'], environment: {}, deploymentTopology: true } },
      components: { app: { requires: [], files: ['images/app.tar'] } },
      files: [{ path: 'images/app.tar', size: 10, sha256: String(index).repeat(64) }],
    }]));
    await writeFile(join(root, 'setup/install.json'), JSON.stringify({ schemaVersion: 2, catalogs }));
    for (const [index, target] of ['k3d-arm64', 'k3s-arm64'].entries()) {
      const catalog = await readInstallCatalog(root, target);
      expect(installPlan(catalog, target, []).files.map(file => file.sha256)).toEqual([String(index).repeat(64)]);
    }
    await expect(readInstallCatalog(root, 'k3s-amd64')).rejects.toThrow('未提供目标');
    await expect(readInstallCatalog(root)).rejects.toThrow('未指定');
    catalogs['k3s-arm64']!.targets = catalogs['k3d-arm64']!.targets;
    await writeFile(join(root, 'setup/install.json'), JSON.stringify({ schemaVersion: 2, catalogs }));
    await expect(readInstallCatalog(root, 'k3s-arm64')).rejects.toThrow('不一致');
  } finally { await rm(root, { recursive: true, force: true }); }
});
