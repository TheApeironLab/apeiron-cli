import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

export async function verifyDeploymentSignature(bytes, signature, publicKey, fingerprint) {
  if (!/^[A-F0-9]{40}$/.test(fingerprint)) throw new Error('Invalid reviewed signing fingerprint');
  const home = await mkdtemp(join(tmpdir(), 'apeiron-signature-'));
  const run = args => {
    const p = Bun.spawnSync(['gpg', '--homedir', home, '--batch', ...args], { stdout: 'pipe', stderr: 'pipe' });
    if (p.exitCode) throw new Error('Deployment signature verification failed');
    return p.stdout.toString();
  };
  try {
    await writeFile(join(home, 'public.asc'), publicKey);
    await writeFile(join(home, 'archive'), bytes);
    await writeFile(join(home, 'archive.asc'), signature);
    run(['--import', join(home, 'public.asc')]);
    const status = run(['--status-fd', '1', '--verify', join(home, 'archive.asc'), join(home, 'archive')]);
    if (!status.split('\n').some(line => line.startsWith(`[GNUPG:] VALIDSIG ${fingerprint} `)))
      throw new Error('Deployment signature does not match the reviewed key');
  } finally { await rm(home, { recursive: true, force: true }); }
}

export async function verifyPublicDeployment(input) {
  const response = await fetch(input.url, { redirect: 'error', signal: AbortSignal.timeout(180_000) });
  if (response.status !== 200) throw new Error('Pinned Chentu archive is not publicly readable');
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (createHash('sha256').update(bytes).digest('hex') !== input.sha256) throw new Error('Public Chentu archive digest mismatch');
  const signature = await fetch(input.url + '.asc', { redirect: 'error', signal: AbortSignal.timeout(30_000) });
  if (signature.status !== 200) throw new Error('Public Chentu signature is missing');
  const recipe = JSON.parse(await readFile(new URL('../../release/recipe.json', import.meta.url), 'utf8'));
  await verifyDeploymentSignature(bytes, await signature.text(),
    await readFile(new URL('../../release/signing-key.asc', import.meta.url)), recipe.signingFingerprint);
}
