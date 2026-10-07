import { expect, test } from 'bun:test';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { includeK3dAirgap, K3D_AIRGAP } from '../src/resources/k3d-airgap';
import { installPlan, type InstallCatalog } from '../src/resources/install';

test('K3d base images enter the resource closure and a mismatched K3s version fails before deployment', async () => {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-airgap-'));
  try {
    await mkdir(join(root, 'stack/k3s'), { recursive: true });
    const versionFile = join(root, 'stack/k3s/artifacts.yaml');
    const catalog: InstallCatalog = { schemaVersion: 1, targets: { 'k3d-arm64': {
      deploymentTopology: true, base: [], environment: {},
    } }, components: {}, files: [] };
    await Bun.write(versionFile, 'version: v1.35.5+k3s1\n');
    await expect(includeK3dAirgap(catalog, root, 'arm64')).rejects.toThrow('版本');
    expect(catalog.files).toHaveLength(0);
    await Bun.write(versionFile, 'version: v1.36.3+k3s1\n');
    await includeK3dAirgap(catalog, root, 'arm64');
    await includeK3dAirgap(catalog, root, 'arm64');
    const plan = installPlan(catalog, 'k3d-arm64', []);
    expect(plan.files).toEqual([K3D_AIRGAP]);
    expect(plan.target.k3sAirgap).toBe(K3D_AIRGAP.path);
    plan.target.k3sAirgap = 'unselected.tar';
    expect(() => installPlan(catalog, 'k3d-arm64', [])).toThrow('校验资源清单');
  } finally { await rm(root, { recursive: true, force: true }); }
});
