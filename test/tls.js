import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
const run = promisify(execFile);
// Short-lived test-only CA and leaf, trusted only by the isolated CLI child.
export async function tlsFixture(dir, fetch) {
  const ca = join(dir, 'test-ca.crt'), key = join(dir, 'test-ca.key');
  const leaf = join(dir, 'test-leaf.crt'), leafKey = join(dir, 'test-leaf.key');
  await run('openssl', ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '1', '-subj', '/CN=CLI test CA only', '-addext', 'basicConstraints=critical,CA:TRUE', '-keyout', key, '-out', ca]);
  await run('openssl', ['req', '-new', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-subj', '/CN=localhost', '-keyout', leafKey, '-out', join(dir, 'leaf.csr')]);
  await writeFile(join(dir, 'leaf.ext'), 'basicConstraints=critical,CA:FALSE\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n');
  await run('openssl', ['x509', '-req', '-in', join(dir, 'leaf.csr'), '-CA', ca, '-CAkey', key, '-CAcreateserial', '-days', '1', '-extfile', join(dir, 'leaf.ext'), '-out', leaf]);
  const server = Bun.serve({ hostname: '127.0.0.1', port: 0, tls: { key: Bun.file(leafKey), cert: Bun.file(leaf) }, fetch });
  return { server, ca, url: `https://127.0.0.1:${server.port}` };
}
