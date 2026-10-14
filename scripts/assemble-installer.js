import { planAssembly, assemble } from './lib/installer-assembly.js';
if (process.argv.includes('--help')) {
  console.log('Usage: bun scripts/assemble-installer.js --bundle DIR --cli DIR --candidate NAME --output NEW_DIR --profile JSON --validator PATH [--wizard-proof JSON] --plan|--assemble\nInputs must be absolute paths. CLI directory contains apeiron and build-provenance.json. See docs/installer-assembly.md.');
  process.exit(0);
}
const options = {};
for (let i = 2; i < process.argv.length; i++) {
  const name = process.argv[i];
  if (name === '--plan' || name === '--assemble') {
    if (options.mode) throw new Error('Choose exactly one of --plan or --assemble');
    options.mode = name.slice(2);
  } else {
    if (!['--bundle', '--cli', '--candidate', '--output', '--profile', '--validator', '--wizard-proof'].includes(name) || !process.argv[i + 1] || process.argv[i + 1].startsWith('--')) throw new Error('Unknown or missing argument: ' + name);
    const key = name.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    if (options[key]) throw new Error('Duplicate argument: ' + name);
    options[key] = process.argv[++i];
  }
}
if (!options.mode) throw new Error('Specify --plan or --assemble');
const plan = await planAssembly(options);
console.log(JSON.stringify(options.mode === 'plan' ? plan : await assemble(options, plan), null, 2));
