import { expect, test } from 'bun:test';
import { validateConfig } from '../src/init/config';
import { environmentFor } from '../src/init/deploy';
import { installationDefaults } from '../src/init/installation';
import { requiredApps } from './fixtures';

test('deployment mode overrides stale Nexus proxy settings and preserves other values on replay', () => {
  for (const topology of ['single-k3s', 'multi-k3s', 'single-k3d'] as const) {
    let source = Bun.YAML.stringify({ topology, releases: {
      nexus: { enabled: true, values: { publicProxies: false, storage: '50Gi', node: 'node-0' } },
      postgres: { enabled: true, values: { storage: '10Gi' } },
    } });
    for (const offline of [false, true, false]) {
      const config = validateConfig({ slug: 'example', apps: requiredApps, deployment: {
        offline, bundleDir: offline ? '/opt/fixture-bundle' : '',
        installation: { ...installationDefaults(), topology,
          domain: 'example.internal', entryIp: topology === 'single-k3d' ? '127.0.0.1' : '192.0.2.10',
          httpPort: 80, httpsPort: 443,
          nodes: topology === 'multi-k3s' ? [
            { host: 'node-0', name: 'node-0', address: '192.0.2.10', role: 'server' },
            { host: 'node-1', name: 'node-1', address: '192.0.2.11', role: 'agent' },
          ] : [],
        },
      } });
      source = environmentFor(config, source);
      const values = Bun.YAML.parse(source) as any;
      expect(values.releases.nexus).toEqual({ enabled: true, values: { publicProxies: !offline, storage: '50Gi', node: 'node-0' } });
      expect(values.releases.postgres).toEqual({ enabled: true, values: { storage: '10Gi' } });
      expect(environmentFor(config, source)).toBe(source);
    }
  }
});
