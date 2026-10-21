import { readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
// Release/backend logic lives in Rust; JS is confined to browser assets and tooling.
const root = fileURLToPath(new URL('../', import.meta.url));
const zygosSubmodule = join(root, 'third_party', 'zygos');
const parser = new Bun.Transpiler({ loader: 'js', target: 'bun' });
let checked = 0;
async function visit(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (['.git', 'node_modules', 'target', 'dist'].includes(entry.name)) continue;
    const path = join(directory, entry.name);
    // Zygos owns its monorepo sources; Apeiron compiles only its Rust CLI crate.
    if (path === zygosSubmodule) continue;
    if (entry.isDirectory()) await visit(path);
    else if (/\.(?:tsx?|mts|cts)$/.test(path)) throw new Error('TypeScript source is not allowed: ' + path);
    else if (path.endsWith('.js')) {
      const { imports } = parser.scan(await Bun.file(path).text());
      for (const dependency of imports) {
        if (dependency.path.startsWith('.')) Bun.resolveSync(dependency.path, directory);
      }
      checked++;
    }
  }
}
await visit(root);
console.log(`PASS ${checked} JavaScript files parsed, relative imports resolved, no TypeScript sources`);
