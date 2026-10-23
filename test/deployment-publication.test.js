import { expect, test } from 'bun:test';
import { publishImmutable } from '../scripts/lib/release-files.js';

const expected = { sha256: 'a'.repeat(64), size: 123 };
const key = `apeiron/blobs/sha256/${expected.sha256}`;
test('deployment upload refuses changed local bytes and never writes a conflicting immutable object', async () => {
  let writes = 0;
  const dependencies = { fileIdentity: async () => expected, publicIdentity: async () => ({ ...expected, size: 124 }),
    run: async () => { writes++; } };
  await expect(publishImmutable('/artifact', key, expected, dependencies)).rejects.toThrow('already exists');
  await expect(publishImmutable('/artifact', key, { ...expected, size: 125 }, dependencies)).rejects.toThrow('changed');
  expect(writes).toBe(0);
});
test('deployment upload reads bytes back, rejects racing writes and preserves exact retries', async () => {
  let reads = 0, writes = 0, command;
  const dependencies = { fileIdentity: async () => expected, publicIdentity: async () => ++reads === 1 ? null : expected,
    run: async args => { writes++; command = args; } };
  expect(await publishImmutable('/artifact', key, expected, dependencies)).toEndWith(key);
  expect(writes).toBe(1);
  expect(reads).toBe(2);
  expect(command).toContain('--forbid-overwrite');
  expect(command.slice(0, 4)).toEqual(['aliyun', 'ossutil', 'api', 'put-object']);
  expect(command).not.toContain('--force');
  await publishImmutable('/artifact', key, expected, dependencies);
  expect(writes).toBe(1);
  reads = 0;
  dependencies.publicIdentity = async () => ++reads === 1 ? null : { ...expected, sha256: 'b'.repeat(64) };
  await expect(publishImmutable('/artifact', key, expected, dependencies)).rejects.toThrow('read-back');
});
test('deployment preparation cannot write download channel pointers', async () => {
  for (const key of ['apeiron-cli/channels/stable.txt', 'apeiron-cli/latest.txt', 'chentu/releases/../bad', 'apeiron/blobs/sha256/not-a-digest'])
    await expect(publishImmutable('/artifact', key, expected)).rejects.toThrow('outside');
});

test('server rejection is idempotent only when the winning object has identical bytes', async () => {
  let reads = 0;
  const dependencies = { fileIdentity: async () => expected,
    publicIdentity: async () => ++reads === 1 ? null : expected,
    run: async () => { throw new Error('server rejected overwrite'); } };
  expect(await publishImmutable('/artifact', key, expected, dependencies)).toEndWith(key);
  reads = 0;
  dependencies.publicIdentity = async () => ++reads === 1 ? null : { ...expected, size: 0 };
  await expect(publishImmutable('/artifact', key, expected, dependencies)).rejects.toThrow('server rejected');
  await expect(publishImmutable('/artifact', 'apeiron/blobs/sha256/' + 'b'.repeat(64), expected, dependencies)).rejects.toThrow('Content address');
});

test('large immutable blobs use conditional multipart upload and still verify public bytes', async () => {
  const large = { ...expected, size: 6 * 1024 ** 3 };
  let reads = 0, command;
  const dependencies = { fileIdentity: async () => large,
    publicIdentity: async () => ++reads === 1 ? null : large,
    run: async args => { command = args; } };
  expect(await publishImmutable('/large-image.tar', key, large, dependencies)).toEndWith(key);
  expect(command[0]).toBe('python3');
  expect(command[1]).toEndWith('/scripts/multipart-deployment-upload.py');
  expect(command).toContain(key);
  expect(command).toContain(String(large.size));
  expect(reads).toBe(2);
});
