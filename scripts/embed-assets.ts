import { renderPage } from '../src/init/page';
import { APPS } from '../src/init/config';

// Committed assets let cargo build work without a JavaScript toolchain. CI checks
// they match the frontend; regenerate after changing the wizard or app catalog.
const assets: Record<string, string> = {
  'setup.html': renderPage('APEIRON_NONCE_PLACEHOLDER'),
  'apps.json': JSON.stringify(APPS),
};
for (const [name, contents] of Object.entries(assets)) {
  const file = Bun.file(new URL(`../runtime/assets/${name}`, import.meta.url));
  if (process.argv.includes('--check')) {
    if (await file.text() !== contents) throw new Error(`${name} is stale; run bun scripts/embed-assets.ts`);
  } else {
    await Bun.write(file, contents);
  }
}
