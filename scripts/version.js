// Cargo owns the version for the executable and all release metadata.
const manifest = Bun.TOML.parse(await Bun.file(new URL('../Cargo.toml', import.meta.url)).text());
export const version = manifest.workspace.package.version;
