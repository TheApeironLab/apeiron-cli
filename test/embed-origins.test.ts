import { expect, test } from 'bun:test';
import { validateConfig } from '../src/init/config';
import { environmentFor } from '../src/init/deploy';
import { installationDefaults } from '../src/init/installation';
import { requiredApps } from './fixtures';

test('iframe origins follow the configured domain across deployments and allow HTTPS ports', () => {
  let source = 'releases: {}\nembedAllowedOrigins: ["https://old.example.com:443"]\n';
  for (const domain of ['alpha.apeironlab.internal', 'beta.apeironlab.internal', 'platform.example.com']) {
    const config = validateConfig({ slug: 'team', apps: requiredApps, deployment: {
      offline: false, installation: { ...installationDefaults(), topology: 'single-k3d', domain, entryIp: '127.0.0.1', httpsPort: 54321 },
    } });
    source = environmentFor(config, source);
    expect((Bun.YAML.parse(source) as any).embedAllowedOrigins).toEqual([
      `https://${domain}:*`, `https://*.${domain}:*`,
    ]);
    expect(environmentFor(config, source)).toBe(source);
  }
});
