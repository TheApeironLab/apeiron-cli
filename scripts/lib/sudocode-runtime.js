import { createHash } from 'node:crypto';
import { readFile, writeFile, realpath } from 'node:fs/promises';
import { join, basename } from 'node:path';
import { execFileSync } from 'node:child_process';

export async function sudocodeSource(root) {
  const directory = join(root, 'third_party/sudocode');
  const git = args => execFileSync('git', args, { cwd: directory, encoding: 'utf8' }).trim();
  if (await realpath(git(['rev-parse', '--show-toplevel'])) !== await realpath(directory))
    throw new Error('Initialize the Sudocode submodule before packaging');
  if (git(['status', '--porcelain', '--untracked-files=all']))
    throw new Error('Sudocode submodule must be clean');
  const commit = git(['rev-parse', 'HEAD']);
  const cargo = Bun.TOML.parse(await readFile(join(directory, 'rust/Cargo.toml'), 'utf8'));
  const version = 'v' + cargo.workspace.package.version;
  if (!/^v\d+\.\d+\.\d+$/.test(version))
    throw new Error('Unsupported Sudocode source version');
  return { version, commit };
}

export function runtimeUrl(value) {
  const url = new URL(value);
  if (url.protocol !== 'https:' || url.username || url.password || url.hash || url.search)
    throw new Error('Sudocode runtime source must be a credential-free HTTPS URL');
  return url;
}

export function verifySudocodeArchive(bytes, pin) {
  if (bytes.length !== pin.archiveSize || createHash('sha256').update(bytes).digest('hex') !== pin.archiveSha256)
    throw new Error('Sudocode archive differs from the reviewed runtime pin');
}

export async function prepareSudocode(root, target, destination) {
  const manifest = JSON.parse(await readFile(join(root, 'release/sudocode-runtime.json'), 'utf8'));
  const recipe = JSON.parse(await readFile(join(root, 'release/recipe.json'), 'utf8'));
  if (manifest.schemaVersion !== 1 || manifest.version !== recipe.sudocode.version || manifest.commit !== recipe.sudocode.commit)
    throw new Error('Sudocode runtime pins must match release/recipe.json');
  const source = await sudocodeSource(root);
  if (manifest.version !== source.version || manifest.commit !== source.commit)
    throw new Error('Sudocode runtime pins must match the submodule; run scripts/pin-sudocode.js');
  const pin = manifest.platforms[target];
  if (!pin) throw new Error('Unsupported Sudocode runtime target');
  const url = runtimeUrl(pin.url);
  let bytes;
  if (process.env.APEIRON_SCODE_ARCHIVE_DIR) {
    bytes = await readFile(join(process.env.APEIRON_SCODE_ARCHIVE_DIR, basename(url.pathname)));
  } else {
    const response = await fetch(pin.url, { signal: AbortSignal.timeout(300_000) });
    if (!response.ok) throw new Error(`Cannot download pinned Sudocode runtime: HTTP ${response.status}`);
    bytes = Buffer.from(await response.arrayBuffer());
  }
  verifySudocodeArchive(bytes, pin);
  const path = join(destination, 'scode.tar.gz');
  await writeFile(path, bytes);
  return { path, version: manifest.version, commit: manifest.commit, ...pin };
}
