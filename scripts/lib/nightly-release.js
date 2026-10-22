import { compareReleaseVersions, nightlyVersion, parseReleaseVersion } from './release-version';

export function planNightly({ base, revision, stable, previous, date, sequence }) {
  if (!/^[a-f0-9]{40}$/.test(revision)) throw new Error('Nightly requires an exact source commit');
  if (stable !== null) {
    if (parseReleaseVersion(stable).channel !== 'stable') throw new Error('Invalid stable pointer');
    if (compareReleaseVersions(base, stable) <= 0)
      return { skip: 'The Cargo target is already stable; advance the base version for the next development cycle.' };
  }
  if (previous !== null) {
    if (previous.schemaVersion !== 2 || parseReleaseVersion(previous.version).channel !== 'nightly'
        || !/^[a-f0-9]{40}$/.test(previous.revision)) throw new Error('Invalid previous nightly manifest');
    if (previous.revision === revision) return { skip: 'This commit already has a successful nightly release.' };
  }
  const version = nightlyVersion(base, date, sequence);
  return { version, tag: `v${version}`, revision };
}
