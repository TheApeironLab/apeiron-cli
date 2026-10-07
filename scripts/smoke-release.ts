import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { version } from '../package.json';

const binary = process.argv[2] && resolve(process.argv[2]);
if (!binary) throw new Error('Usage: bun run test:release /path/to/apeiron');
const check = Bun.spawn([binary, '--version'], { stdout: 'pipe', stderr: 'pipe' });
assert.equal((await new Response(check.stdout).text()).trim(), version);
assert.equal(await check.exited, 0);
const directory = await mkdtemp(join(tmpdir(), 'apeiron-release-smoke-'));
const child = spawn(binary, ['init', '--no-open', '--config', join(directory, 'config.json')], {
  env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: directory }, stdio: ['ignore', 'pipe', 'pipe'],
});
const exited = new Promise<number | null>((done, reject) => { child.once('error', reject); child.once('close', done); });
try {
  const url = await new Promise<string>((done, reject) => {
    const timer = setTimeout(() => reject(new Error('Compiled setup did not start')), 15000);
    let output = '';
    child.stdout.on('data', data => {
      output += data.toString();
      const found = /\nurl\t(http:\/\/127\.0\.0\.1:\d+\/setup\/[a-f0-9]+\/)\n/.exec(output);
      if (found) { clearTimeout(timer); done(found[1]!); }
    });
    child.once('close', () => { clearTimeout(timer); reject(new Error('Compiled setup exited early')); });
  });
  const page = await fetch(url); assert.equal(page.status, 200);
  assert.match(await page.text(), /href="\.\/favicon.ico"/);
  const icon = await fetch(url + 'favicon.ico'); assert.equal(icon.status, 200);
  assert.deepEqual([...new Uint8Array(await icon.arrayBuffer()).slice(0, 4)], [0, 0, 1, 0]);
  const response = await fetch(url + 'api/config'); assert.equal(response.status, 200);
  const config = await response.json() as { deployment: { phase: string } };
  assert.equal(config.deployment.phase, 'idle');
  console.log(`PASS standalone ${version}: version, setup, embedded favicon and configuration API; Bun absent from PATH`);
} finally {
  child.kill('SIGINT');
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
  try { await exited; } finally { clearTimeout(timer); await rm(directory, { recursive: true, force: true }); }
}
