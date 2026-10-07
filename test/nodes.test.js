import { expect, test } from 'bun:test';
import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
test('node probe excludes inactive Bluetooth addresses and keeps active Tailscale', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'apeiron-node-probe-'));
    const before = process.env.PATH;
    try {
        const interfaces = [
            { ifname: 'pan0', flags: ['BROADCAST'], addr_info: [{ family: 'inet', local: '192.0.2.1' }] },
            { ifname: 'eth0', flags: ['UP'], addr_info: [{ family: 'inet', local: '192.0.2.2' }] },
            { ifname: 'tailscale0', flags: ['UP'], operstate: 'UNKNOWN', addr_info: [{ family: 'inet', local: '100.64.0.2' }] },
            { ifname: 'docker0', flags: ['UP'], addr_info: [{ family: 'inet', local: '192.0.2.3' }] },
        ];
        await writeFile(join(dir, 'ip'), "#!/bin/sh\ncat <<'DATA'\n" + JSON.stringify(interfaces) + '\nDATA\n');
        await chmod(join(dir, 'ip'), 0o700);
        process.env.PATH = `${dir}:${before}`;
        const { stdout } = await promisify(execFile)('python3', [new URL('../runtime/assets/node-probe.py', import.meta.url).pathname], { env: { ...process.env, PATH: `${dir}:${before}` } });
        const facts = JSON.parse(stdout);
        expect(facts.addresses).toEqual(['192.0.2.2', '100.64.0.2']);
    }
    finally {
        process.env.PATH = before;
        await rm(dir, { recursive: true, force: true });
    }
});
