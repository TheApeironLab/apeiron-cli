import { test, expect } from 'bun:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { verifyDeploymentSignature } from '../scripts/lib/deployment-signature.js';

test('deployment signatures bind the exact bytes and reviewed signing identity', async () => {
  const home = await mkdtemp(join(tmpdir(), 'release-key-test-'));
  const run = args => {
    const result = Bun.spawnSync(['gpg', '--homedir', home, '--batch', ...args], { stdout: 'pipe', stderr: 'pipe' });
    if (result.exitCode) throw new Error(result.stderr.toString());
    return result.stdout;
  };
  try {
    run(['--pinentry-mode', 'loopback', '--passphrase', '', '--quick-generate-key',
      'Release Test <release@example.invalid>', 'rsa2048', 'sign', '1d']);
    const fingerprint = run(['--with-colons', '--list-keys']).toString().split('\n').find(x => x.startsWith('fpr:')).split(':')[9];
    const key = run(['--armor', '--export', fingerprint]);
    await writeFile(join(home, 'archive'), 'release bytes');
    run(['--armor', '--local-user', fingerprint + '!', '--detach-sign', join(home, 'archive')]);
    const signature = await readFile(join(home, 'archive.asc'));
    await verifyDeploymentSignature('release bytes', signature, key, fingerprint);
    await expect(verifyDeploymentSignature('changed bytes', signature, key, fingerprint)).rejects.toThrow('verification failed');
    await expect(verifyDeploymentSignature('release bytes', signature, key, 'A'.repeat(40))).rejects.toThrow('reviewed key');
    await expect(verifyDeploymentSignature('release bytes', 'not a signature', key, fingerprint)).rejects.toThrow('verification failed');
  } finally {
    Bun.spawnSync(['gpgconf', '--homedir', home, '--kill', 'all']);
    await rm(home, { recursive: true, force: true });
  }
});
