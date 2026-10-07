import { spawn } from 'node:child_process';
import { startInitServer as legacyStart } from '../src/init/server';

// Replay the existing HTTP assertions against an actual Rust child. Tests that
// inject in-process TS functions remain reference tests, alongside Rust boundary tests.
export const startInitServer: typeof legacyStart = async options => {
  if (!process.env.APEIRON_TEST_BIN || options.resources || options.gateway) return legacyStart(options);
  const child = spawn(process.env.APEIRON_TEST_BIN, ['init', '--no-open', '--config', options.path, ...(options.port === undefined ? [] : ['--port', String(options.port)])], { stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '', errors = '', result: any = { phase: 'idle' };
  child.stderr.on('data', chunk => { errors += chunk; });
  const closed = new Promise<void>(resolve => child.once('close', () => {
    const phase = [...output.matchAll(/^deployment\t([^\n]+)/gm)].at(-1)?.[1];
    if (phase) result = { ...result, phase };
    resolve();
  }));
  const url = await new Promise<string>((resolve, reject) => {
    const timer = setTimeout(() => { child.kill(); reject(new Error('Rust wizard startup timed out')); }, 10000);
    child.stdout.on('data', chunk => { output += chunk; const url = /^url\t(.+)$/m.exec(output)?.[1]; if (url) { clearTimeout(timer); resolve(url); } });
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', () => { clearTimeout(timer); reject(new Error(errors)); });
  });
  let polling = false;
  const timer = setInterval(async () => { if (polling) return; polling = true; try { result = await fetch(url + 'api/deployment').then(r => r.json()); } catch {} finally { polling = false; } }, 10);
  void closed.then(() => clearInterval(timer));
  return { url, origin: new URL(url).origin, closed, get result() { return result; }, stop: async () => {
    child.kill('SIGINT'); const kill = setTimeout(() => child.kill('SIGKILL'), 20000); try { await closed; } finally { clearTimeout(kill); }
  } };
};
