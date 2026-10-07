#!/usr/bin/env bun
import { existsSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { runInit } from '../src/init/command';
import { version } from '../package.json';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const [module, ...args] = process.argv.slice(2);
function fail(message: string, code = 2): never {
  console.error(`schema: apeiron.v1\nerror: ${message}`);
  process.exit(code);
}
if (!module || ['--help', '-h', 'describe'].includes(module)) {
  console.log('schema=apeiron.v1\ncommand\tusage\ninit\tapeiron init [--port N] [--config path] [--no-open] — local configuration wizard\nonto\tapeiron onto <command> [flags]\n<module>\tAPEIRON_<MODULE>_BIN=/path/to/cli apeiron <module> <command>\nstatus\tShow ontology checkout and configured entry point\nverify\tCheck ontology CLI entry point\n--version\tShow version');
  process.exit(0);
}
if (module === '--version') { console.log(version); process.exit(0); }
if (module === 'init') process.exit(await runInit(args));
const ontologyRoot = resolve(process.env.APEIRON_ONTO_ROOT || resolve(root, '../ontology'));
const ontoEntry = resolve(ontologyRoot, 'apps/onto/cli/main.ts');
if (module === 'status' || module === 'verify') {
  const present = existsSync(ontoEntry);
  console.log(`schema=apeiron.v1\nmodule\tentry\tavailable\nonto\t${ontoEntry}\t${present}`);
  process.exit(module === 'verify' && !present ? 4 : 0);
}
if (!/^[a-z][a-z0-9-]*$/.test(module)) fail('Invalid module name');
const configured = process.env[`APEIRON_${module.toUpperCase().replaceAll('-', '_')}_BIN`];
let command: string;
let forwarded: string[];
let cwd: string | undefined;
if (configured) {
  command = configured;
  forwarded = args;
} else if (module === 'onto') {
  if (!existsSync(ontoEntry)) fail('Ontology checkout missing; set APEIRON_ONTO_ROOT to its repository path', 4);
  command = process.env.APEIRON_BUN_BIN || 'bun';
  forwarded = [ontoEntry, ...args];
  cwd = ontologyRoot;
} else {
  fail(`Module ${module} is not configured; set APEIRON_${module.toUpperCase().replaceAll('-', '_')}_BIN to its executable`, 4);
}
const child = spawn(command, forwarded, { cwd, stdio: 'inherit', env: process.env });
child.on('error', error => fail(`Cannot start ${module}: ${error.message}`, 4));
for (const signal of ['SIGINT', 'SIGTERM'] as const) process.on(signal, () => child.kill(signal));
child.on('exit', (code, signal) => {
  process.exit(code ?? (signal === 'SIGINT' ? 130 : signal === 'SIGTERM' ? 143 : 9));
});
