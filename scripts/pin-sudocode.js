import { readFile, writeFile } from 'node:fs/promises';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { sha256 } from './lib/release-provenance.js';
import { runtimeUrl, sudocodeSource, verifySudocodeArchive } from './lib/sudocode-runtime.js';

// Generate reviewable pins from the selected submodule and its published binaries.
// All four archives and the release's SHA256SUMS.txt must be prepared externally.
export async function pinSudocode(root, archives, baseUrl) {
  if (!isAbsolute(archives) || resolve(archives).startsWith(resolve(root) + '/') || resolve(archives) === resolve(root))
    throw new Error('Sudocode archives must be in an absolute directory outside the checkout');
  const base = runtimeUrl(baseUrl.endsWith('/') ? baseUrl : baseUrl + '/');
  const source = await sudocodeSource(root);
  const tag = execFileSync('git', ['rev-parse', `refs/tags/${source.version}^{commit}`], {
    cwd: join(root, 'third_party/sudocode'), encoding: 'utf8',
  }).trim();
  if (tag !== source.commit) throw new Error('Prebuilt Sudocode packaging requires the matching release tag');
  const checksums = new Map();
  for (const line of (await readFile(join(archives, 'SHA256SUMS.txt'), 'utf8')).split('\n')) {
    const match = line.match(/^([a-f0-9]{64})\s+\*?(\S+)$/);
    if (match) checksums.set(match[2], match[1]);
  }
  const platforms = {};
  for (const target of ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64']) {
    const name = 'scode-' + target.replace('darwin', 'macos');
    const archive = join(archives, name + '.tar.gz');
    const bytes = await readFile(archive);
    verifySudocodeArchive(bytes, { archiveSize: bytes.length, archiveSha256: checksums.get(name + '.tar.gz') });
    const member = name + '/scode';
    const entries = execFileSync('tar', ['-tvf', archive], { encoding: 'utf8' }).trim().split('\n');
    const matches = entries.filter(line => line.endsWith(' ' + member));
    if (matches.length !== 1 || !matches[0].startsWith('-'))
      throw new Error('Sudocode archive must contain one regular executable: ' + member);
    const binary = execFileSync('tar', ['-xOf', archive, member], { maxBuffer: 128 * 1024 * 1024 });
    if (!binary.length) throw new Error('Empty Sudocode executable');
    platforms[target] = {
      url: new URL(name + '.tar.gz', base).href,
      archiveSha256: sha256(bytes), archiveSize: bytes.length, member,
      binarySha256: sha256(binary), binarySize: binary.length,
    };
  }
  const recipePath = join(root, 'release/recipe.json');
  const recipe = JSON.parse(await readFile(recipePath, 'utf8'));
  recipe.sudocode = source;
  // Validate every asset before changing either tracked file.
  await writeFile(join(root, 'release/sudocode-runtime.json'), JSON.stringify({ schemaVersion: 1, ...source, platforms }, null, 2) + '\n');
  await writeFile(recipePath, JSON.stringify(recipe, null, 2) + '\n');
  return source;
}

if (import.meta.main) {
  const [archives, baseUrl, ...extra] = process.argv.slice(2);
  if (!archives || !baseUrl || extra.length)
    throw new Error('Usage: bun scripts/pin-sudocode.js /external/archives https://download-host/version-directory/');
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  console.log(await pinSudocode(root, archives, baseUrl));
}
