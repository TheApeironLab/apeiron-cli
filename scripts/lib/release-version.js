const core = '(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)';
const pattern = new RegExp(`^${core}(?:-nightly\\.([0-9]{8})\\.([1-9][0-9]*))?$`);

export function parseReleaseVersion(version) {
  const match = typeof version === 'string' && pattern.exec(version);
  if (!match || match[0] !== version) throw new Error(`Invalid release version: ${version}`);
  const [, major, minor, patch, date, sequence] = match;
  if (date) {
    const iso = `${date.slice(0, 4)}-${date.slice(4, 6)}-${date.slice(6, 8)}`;
    const parsed = new Date(`${iso}T00:00:00Z`);
    if (!Number.isFinite(parsed.getTime()) || parsed.toISOString().slice(0, 10) !== iso)
      throw new Error('Invalid nightly date');
  }
  return { base: `${major}.${minor}.${patch}`, channel: date ? 'nightly' : 'stable',
    numbers: [major, minor, patch].map(BigInt), date, sequence };
}

export function resolveReleaseVersion(base, ref = '', override = '') {
  if (parseReleaseVersion(base).channel !== 'stable') throw new Error('Cargo version must be a base version');
  const tagged = ref.startsWith('refs/tags/') ? ref.slice('refs/tags/v'.length) : '';
  if (ref.startsWith('refs/tags/') && (!ref.startsWith('refs/tags/v') || !tagged)) throw new Error('Release tags must start with v and contain a version');
  const version = override || tagged || base;
  if (parseReleaseVersion(version).base !== base || (tagged && tagged !== version))
    throw new Error('Release version must match the Cargo base version and tag');
  return version;
}

export function compareReleaseVersions(left, right) {
  const a = parseReleaseVersion(left), b = parseReleaseVersion(right);
  for (let i = 0; i < 3; i++) {
    if (a.numbers[i] !== b.numbers[i]) return a.numbers[i] > b.numbers[i] ? 1 : -1;
  }
  if (a.channel !== b.channel) return a.channel === 'stable' ? 1 : -1;
  if (a.channel === 'stable') return 0;
  for (const key of ['date', 'sequence']) {
    if (BigInt(a[key]) !== BigInt(b[key])) return BigInt(a[key]) > BigInt(b[key]) ? 1 : -1;
  }
  return 0;
}

export function mayAdvanceChannel(version, current, channel) {
  if (parseReleaseVersion(version).channel !== channel) throw new Error('Release/channel mismatch');
  if (current === null) return true;
  if (parseReleaseVersion(current).channel !== channel) throw new Error('Invalid channel pointer');
  return compareReleaseVersions(version, current) >= 0;
}

export function nightlyVersion(base, date, sequence) {
  const version = `${base}-nightly.${date}.${sequence}`;
  resolveReleaseVersion(base, '', version);
  return version;
}
