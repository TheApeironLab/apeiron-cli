// Run every Chentu-owned HTTP contract against the embedded Apeiron entry.
// No assertions are duplicated or removed when the upstream suite grows.
import { readdir } from 'node:fs/promises';
import { resolve } from 'node:path';

process.env.CHENTU_EMBEDDED = '1';
process.env.CHENTU_TEST_BIN = process.env.APEIRON_TEST_BIN || resolve(process.env.CARGO_TARGET_DIR || new URL('../target', import.meta.url).pathname, 'debug/apeiron');
const directory = new URL('../third_party/chentu/rust/test/', import.meta.url);
for (const name of (await readdir(directory)).filter(name => name.endsWith('.test.js')).sort()) {
  await import(new URL(name, directory).href);
}
