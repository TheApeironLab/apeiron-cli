import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join, basename } from 'node:path';

export function verifySudocodeArchive(bytes, pin) {
  if (bytes.length !== pin.archiveSize || createHash('sha256').update(bytes).digest('hex') !== pin.archiveSha256)
    throw new Error('Sudocode archive differs from the reviewed runtime pin');
}

export async function prepareSudocode(root, target, destination) {
  const manifest = JSON.parse(await readFile(join(root, 'release/sudocode-runtime.json'), 'utf8'));
  const recipe = JSON.parse(await readFile(join(root, 'release/recipe.json'), 'utf8'));
  if (manifest.schemaVersion !== 1 || manifest.version !== recipe.sudocode.version || manifest.commit !== recipe.sudocode.commit)
    throw new Error('Sudocode runtime pins must match release/recipe.json');
  const pin = manifest.platforms[target];
  if (!pin || !pin.url.startsWith('https://github.com/sudoprivacy/sudocode/releases/download/' + manifest.version + '/'))
    throw new Error('Unsupported Sudocode runtime target or source');
  let bytes;
  if (process.env.APEIRON_SCODE_ARCHIVE_DIR) {
    bytes = await readFile(join(process.env.APEIRON_SCODE_ARCHIVE_DIR, basename(pin.url)));
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
