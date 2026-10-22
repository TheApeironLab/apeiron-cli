import { expect, test } from 'bun:test';
import { parseReleaseVersion, resolveReleaseVersion, compareReleaseVersions, mayAdvanceChannel, nightlyVersion } from '../scripts/lib/release-version';
import { planNightly } from '../scripts/lib/nightly-release';

test('release version binds tags and overrides to the Cargo base', () => {
  expect(resolveReleaseVersion('0.2.0')).toBe('0.2.0');
  const version = nightlyVersion('0.2.0', '20261022', '123');
  expect(resolveReleaseVersion('0.2.0', `refs/tags/v${version}`)).toBe(version);
  expect(resolveReleaseVersion('0.2.0', 'refs/pull/12/merge', version)).toBe(version);
  for (const args of [['0.2.0', 'refs/tags/v0.3.0'], ['0.2.0', 'refs/tags/v0.2.0', version],
    ['0.2.0', 'refs/tags/0.2.0'], ['0.2.0', 'refs/tags/v'], ['0.2.0-rc.1'], ['0.2.0', '', '0.3.0-nightly.20261022.1']]) {
    expect(() => resolveReleaseVersion(...args)).toThrow();
  }
});

test('invalid versions cannot become paths, shell values or build environment entries', () => {
  for (const version of ['../0.2.0', '0.02.0', '0.2.0-rc.1', '0.2.0+meta', '0.2.0\n', '0.2.0\nINJECT=1',
    '0.2.0-nightly.20260230.1', '0.2.0-nightly.20261022.01', '0.2.0-nightly.20261022.0']) {
    expect(() => parseReleaseVersion(version)).toThrow();
  }
  expect(parseReleaseVersion('0.2.0-nightly.20280229.1').channel).toBe('nightly');
});

test('channel advancement prevents rollback, cross-channel promotion and lexical ordering errors', () => {
  expect(mayAdvanceChannel('0.2.0', null, 'stable')).toBe(true);
  expect(mayAdvanceChannel('0.2.0', '0.2.0', 'stable')).toBe(true); // interrupted update can recover
  expect(mayAdvanceChannel('0.2.0', '0.2.1', 'stable')).toBe(false);
  expect(mayAdvanceChannel('0.2.0-nightly.20261022.9', '0.2.0-nightly.20261022.10', 'nightly')).toBe(false);
  expect(compareReleaseVersions('0.2.0-nightly.20261022.10', '0.2.0-nightly.20261022.9')).toBe(1);
  expect(compareReleaseVersions('0.2.0', '0.2.0-nightly.20261022.10')).toBe(1);
  expect(compareReleaseVersions('0.10.0', '0.9.0')).toBe(1);
  expect(() => mayAdvanceChannel('0.2.0-nightly.20261022.1', '0.2.0', 'stable')).toThrow();
  expect(() => mayAdvanceChannel('0.2.0', '0.2.0-nightly.20261022.1', 'stable')).toThrow();
});

test('nightly skips successful unchanged source and already stable targets, but retries unpublished source', () => {
  const input = { base: '0.2.0', revision: 'a'.repeat(40), stable: null, previous: null, date: '20261022', sequence: '321' };
  expect(planNightly(input).version).toBe('0.2.0-nightly.20261022.321');
  const previous = { schemaVersion: 2, version: '0.2.0-nightly.20261021.1', revision: input.revision };
  expect(planNightly({ ...input, previous }).skip).toContain('already has');
  expect(planNightly({ ...input, previous: { ...previous, revision: 'b'.repeat(40) } }).version).toBeDefined();
  expect(planNightly({ ...input, stable: '0.2.0' }).skip).toContain('already stable');
  expect(planNightly({ ...input, stable: '0.3.0' }).skip).toBeDefined();
  expect(() => planNightly({ ...input, previous: { ...previous, revision: 'invalid' } })).toThrow();
});
