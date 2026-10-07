import { join } from 'node:path';
import { ConfigError } from '../init/config';
import type { InstallCatalog } from './install';

// Compatibility supplement for the pinned rc.5 catalog. Size and digest are
// fixed from the upstream K3s release, never learned from downloaded image bytes.
export const K3D_AIRGAP = {
  path: 'k3s/k3s-airgap-images-arm64.tar.zst',
  size: 241933732,
  sha256: '856feb047453cd697c2bc4dcf20127448554c8366be992a211558a13d830a038',
  url: 'https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com/chentu/releases/0.1.0-rc.5/k3s/k3s-airgap-images-arm64.tar.zst',
};

export async function includeK3dAirgap(catalog: InstallCatalog, root: string, architecture: string) {
  const target = catalog.targets[`k3d-${architecture}`];
  if (!target) throw new ConfigError('安装包不支持所选 K3d 架构。');
  if (target.k3sAirgap) {
    const declared = catalog.files.find(file => file.path === target.k3sAirgap);
    if (declared?.path === K3D_AIRGAP.path && declared.sha256 === K3D_AIRGAP.sha256 && declared.size === K3D_AIRGAP.size && !declared.url) declared.url = K3D_AIRGAP.url;
    return;
  }
  if (architecture !== 'arm64') throw new ConfigError('当前 K3d 基础镜像包仅支持 ARM64。');
  const artifacts = Bun.YAML.parse(await Bun.file(join(root, 'stack/k3s/artifacts.yaml')).text()) as { version: string };
  if (artifacts.version !== 'v1.36.3+k3s1') throw new ConfigError('K3s 版本与 CLI 固定的基础镜像包不匹配，请更新兼容的资源清单。');
  const existing = catalog.files.find(file => file.path === K3D_AIRGAP.path);
  if (existing && (existing.size !== K3D_AIRGAP.size || existing.sha256 !== K3D_AIRGAP.sha256)) throw new ConfigError('K3d 基础镜像包声明冲突。');
  if (!existing) catalog.files.push({ ...K3D_AIRGAP });
  catalog.components['k3d-airgap'] = { requires: [], files: [K3D_AIRGAP.path] };
  if (!target.base.includes('k3d-airgap')) target.base.push('k3d-airgap');
  target.k3sAirgap = K3D_AIRGAP.path;
}
