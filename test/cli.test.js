import { test } from 'bun:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { binary } from './support';
function run(args, env = {}) {
    return spawnSync(binary, args, { encoding: 'utf8', env: { ...process.env, ...env } });
}
test('help and unknown module', () => {
    assert.equal(run(['--help']).status, 0);
    assert.equal(run(['unconfigured']).status, 4);
});
test('dispatch preserves literal arguments, cwd, output and exit code', () => {
    const dir = mkdtempSync(join(tmpdir(), 'apeiron-cli-'));
    const entry = join(dir, 'module');
    writeFileSync(entry, `#!${process.execPath}\nconsole.log(JSON.stringify(process.argv.slice(2))); process.exit(7);\n`, { mode: 0o755 });
    try {
        const args = ['hello world', '$(echo unexpected)', '--flag=x'];
        const result = run(['wlk', ...args], { APEIRON_WLK_BIN: entry });
        assert.equal(result.status, 7);
        assert.deepEqual(JSON.parse(result.stdout), args);
    }
    finally {
        rmSync(dir, { recursive: true, force: true });
    }
});
test('missing ontology reports failure', () => {
    assert.equal(run(['verify'], { APEIRON_ONTO_ROOT: '/nonexistent/apeiron-ontology' }).status, 4);
});
