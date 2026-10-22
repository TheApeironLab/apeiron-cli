import { parseReleaseVersion, resolveReleaseVersion } from './lib/release-version';
// Cargo owns the base version; immutable nightly tags supply the build suffix.
const manifest = Bun.TOML.parse(await Bun.file(new URL('../Cargo.toml', import.meta.url)).text());
export const baseVersion = manifest.workspace.package.version;
export const version = resolveReleaseVersion(baseVersion, process.env.GITHUB_REF, process.env.APEIRON_BUILD_VERSION);
export const channel = parseReleaseVersion(version).channel;
