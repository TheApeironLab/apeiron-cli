// Every name is derived from an exclusive config directory and a unique run
// domain. K3d registries are detached and do not have a k3d.cluster label.
export async function cleanupK3dLease(cluster, docker) {
  if (!/^apeiron-[a-f0-9]{12}$/.test(cluster)) throw new Error('Invalid acceptance cluster identity');
  const containers = await docker(['ps', '-aq', '--filter', 'label=app=k3d', '--filter', `label=k3d.cluster=${cluster}`]);
  const registry = await docker(['ps', '-aq', '--filter', `name=^/k3d-${cluster}-reg$`,
    '--filter', 'label=app=k3d', '--filter', 'label=k3d.role=registry']);
  if (containers.length || registry.length) await docker(['rm', '-f', '-v', ...new Set([...containers, ...registry])]);
  const networks = await docker(['network', 'ls', '-q', '--filter', 'label=app=k3d', '--filter', `name=^k3d-${cluster}$`]);
  if (networks.length) await docker(['network', 'rm', ...networks]);
  const volumes = await docker(['volume', 'ls', '-q', '--filter', 'label=app=k3d', '--filter', `label=k3d.cluster=${cluster}`]);
  if (volumes.length) await docker(['volume', 'rm', ...volumes]);
}
