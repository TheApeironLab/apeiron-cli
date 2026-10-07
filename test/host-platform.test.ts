import { expect, test } from 'bun:test';
import { supportsK3sHost } from '../src/init/host-platform';
import { validateNativeHostPlatform, type InstallTarget } from '../src/resources/install';

test('native K3s accepts Ubuntu LTS on AMD64 and ARM64, including Spark host facts', () => {
  for (const os of ['Ubuntu', 'ubuntu']) {
    for (const version of ['22.04', '22.04.5', '24.04', '24.04.4']) {
      for (const architecture of version.startsWith('22.') ? ['amd64', 'x64', 'x86_64'] : ['arm64', 'aarch64']) {
        expect(supportsK3sHost(os, version, architecture)).toBe(true);
      }
    }
  }
  for (const [os, version, architecture] of [
    ['macOS', '26.6.2', 'arm64'], ['debian', '12', 'aarch64'],
    ['ubuntu', '20.04', 'x86_64'], ['ubuntu', '24.10', 'aarch64'],
    ['ubuntu', '24.040', 'aarch64'], ['ubuntu', '24.04', 'armv7l'],
    ['ubuntu', '22.04', 'arm64'], ['ubuntu', '24.04', 'amd64'],
  ]) expect(supportsK3sHost(os!, version!, architecture!)).toBe(false);
});

test('native packages must match every node OS version and CPU before installation', () => {
  const target: InstallTarget = { base: [], environment: {}, hostPlatform: { os: 'ubuntu', version: '24.04', architecture: 'arm64' } };
  const spark = { os: 'ubuntu', version: '24.04', architecture: 'aarch64' };
  expect(() => validateNativeHostPlatform(target, [spark])).not.toThrow();
  expect(() => validateNativeHostPlatform(target, [{ ...spark, version: '24.04.4', architecture: 'arm64' }])).not.toThrow();
  expect(() => validateNativeHostPlatform({ base: [], environment: {} }, [spark])).toThrow('hostPlatform');
  expect(() => validateNativeHostPlatform(target, [spark, { ...spark, version: '22.04' }])).toThrow('不匹配');
  expect(() => validateNativeHostPlatform(target, [{ ...spark, architecture: 'x86_64' }])).toThrow('不匹配');
});
