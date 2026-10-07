import { test, expect } from 'bun:test';
import { mkdtemp, writeFile, rm, readFile, chmod, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const cli = new URL('../bin/apeiron.ts', import.meta.url).pathname;
async function fixture(work: (f: { run: (args: string[]) => Promise<{ code: number; out: string; err: string }>; requests: { method: string; path: string; body: any }[]; dir: string; setMode: (mode: string) => void }) => Promise<void>) {
  const dir = await mkdtemp(join(tmpdir(), 'apeiron-chat-')); const token = join(dir, 'token');
  await writeFile(token, 'test-secret-do-not-print', { mode: 0o600 });
  const requests: { method: string; path: string; body: any }[] = []; let mode = '';
  const server = Bun.serve({ hostname: '127.0.0.1', port: 0, async fetch(req) {
    expect(req.headers.get('authorization')).toBe('Bearer test-secret-do-not-print');
    const url = new URL(req.url); const body = req.method === 'GET' ? undefined : await req.json();
    requests.push({ method: req.method, path: url.pathname + url.search, body });
    if (mode === 'redirect') return Response.redirect('http://127.0.0.1:1/stolen');
    if (mode === 'rate') return Response.json({ error: 'test-secret-do-not-print' }, { status: 429, headers: { 'retry-after': '3' } });
    if (mode === 'unauthorized') return Response.json({ error: 'test-secret-do-not-print' }, { status: 401 });
    if (url.pathname.endsWith('/state')) return Response.json(mode === 'encrypted' ? [{ type: 'm.room.encryption' }] : []);
    if (url.pathname.endsWith('/whoami')) return Response.json({ user_id: '@alice:test', device_id: 'TEST' });
    if (url.pathname.endsWith('/search')) return Response.json({ results: [{ user_id: '@bob:test', display_name: 'Bob' }], limited: false });
    if (url.pathname.endsWith('/createRoom')) return Response.json({ room_id: '!room:test' });
    if (url.pathname.endsWith('/joined_rooms')) return Response.json({ joined_rooms: ['!room:test'] });
    if (url.pathname.endsWith('/messages')) return Response.json({ chunk: [{ event_id: '$1', sender: '@bob:test', type: 'm.room.message', content: { body: 'hello\nworld' } }], end: 'next cursor' });
    if (url.pathname.endsWith('/sync')) return Response.json({ next_batch: 's123', rooms: { join: {} }, to_device: { events: [] } });
    return Response.json({ event_id: '$event' });
  } });
  const run = async (args: string[]) => {
    const process = Bun.spawn([Bun.which('bun')!, cli, 'chat', ...args], {
      env: { ...globalThis.process.env, APEIRON_CHAT_SERVER: server.url.origin, APEIRON_CHAT_TOKEN_FILE: token }, stdout: 'pipe', stderr: 'pipe',
    });
    const [out, err, code] = await Promise.all([new Response(process.stdout).text(), new Response(process.stderr).text(), process.exited]);
    return { out, err, code };
  };
  try { await work({ run, requests, dir, setMode: value => { mode = value; } }); }
  finally { server.stop(true); await rm(dir, { recursive: true, force: true }); }
}

test('chat discovers identity, users, private room membership and history via Matrix HTTP', async () => fixture(async ({ run, requests }) => {
  expect((await run(['whoami', '--jsonl'])).out).toContain('@alice:test');
  expect((await run(['user', 'search', '--query', 'Bob'])).code).toBe(0);
  expect(requests.at(-1)?.body).toEqual({ search_term: 'Bob', limit: 10 });
  expect((await run(['room', 'create', '--user', '@bob:test'])).code).toBe(0);
  expect(requests.at(-1)?.body).toEqual({ preset: 'private_chat', is_direct: true, invite: ['@bob:test'] });
  for (const verb of ['join', 'leave', 'invite', 'read']) {
    const more = verb === 'invite' ? ['--user', '@bob:test'] : verb === 'read' ? ['--event', '$event'] : [];
    expect((await run(['room', verb, '--room', '!room:test', ...more])).code).toBe(0);
  }
  expect((await run(['room', 'list'])).out).toContain('!room:test');
  const history = await run(['message', 'list', '--room', '!room:test', '--cursor', 'a&b', '--limit', '3']);
  expect(history.code).toBe(0); expect(history.out).toContain('hello world');
  expect(requests.at(-1)?.path).toContain('from=a%26b'); expect(history.out).toContain('next cursor');
}));

test('send and edit preserve literal text, encode paths and reuse explicit transaction IDs', async () => fixture(async ({ run, requests }) => {
  const args = ['message', 'send', '--room', '!r:test', '--text', '你好 $(echo no)\nline', '--txn-id', 'same/id'];
  for (let i = 0; i < 2; i++) expect((await run(args)).code).toBe(0);
  const sent = requests.filter(r => r.method === 'PUT');
  expect(sent.length).toBe(2); expect(sent[0]).toEqual(sent[1]);
  expect(sent[0]!.path).toContain('same%2Fid'); expect(sent[0]!.body.body).toBe('你好 $(echo no)\nline');
  expect((await run(['message', 'edit', '--room', '!r:test', '--event', '$1', '--text', 'edited', '--txn-id', 'edit-1'])).code).toBe(0);
  expect(requests.at(-1)?.body['m.relates_to']).toEqual({ rel_type: 'm.replace', event_id: '$1' });
  expect((await run(['message', 'redact', '--room', '!r:test', '--event', '$1', '--txn-id', 'redact-1'])).code).toBe(0);
}));

test('invalid commands and dry runs have no network effects', async () => fixture(async ({ run, requests }) => {
  for (const args of [ ['message', 'send', '--room', '!r', '--text', 'x'], ['room', 'list', '--text', 'ignored'], ['sync'], ['message', 'list', '--room', '!r', '--limit', '101'] ]) expect((await run(args)).code).toBe(2);
  const dry = await run(['room', 'create', '--user', '@b:test', '--dry-run', '--jsonl']);
  expect(dry.code).toBe(0); expect(JSON.parse(dry.out).body.invite).toEqual(['@b:test']); expect(requests).toHaveLength(0);
}));

test('encryption and HTTP errors fail closed; credential and upstream text never printed', async () => fixture(async ({ run, requests, setMode }) => {
  setMode('encrypted');
  const denied = await run(['message', 'send', '--room', '!r', '--text', 'private', '--txn-id', 't1']);
  expect(denied.code).toBe(2); expect(requests.every(r => r.method === 'GET')).toBe(true);
  for (const [mode, code] of [['rate', 8], ['unauthorized', 7], ['redirect', 9]] as const) {
    setMode(mode); const result = await run(['status']); expect(result.code).toBe(code);
    expect(result.out + result.err).not.toContain('test-secret-do-not-print');
  }
}));

test('sync persists full response and cursor in a private file without overwriting', async () => fixture(async ({ run, dir, requests }) => {
  const path = join(dir, 'sync.json');
  const result = await run(['sync', '--cursor', 's&0', '--out', path]);
  expect(result.code).toBe(0); expect(requests.at(-1)?.path).toContain('since=s%260');
  const data = JSON.parse(await readFile(path, 'utf8')); expect(data.next_cursor).toBe('s123'); expect(data.rooms).toEqual({ join: {} });
  expect((await run(['sync', '--out', path])).code).toBe(5);
}));

test('credential file permissions and symlinks rejected before requests', async () => fixture(async ({ run, dir, requests }) => {
  await chmod(join(dir, 'token'), 0o644); expect((await run(['status'])).code).toBe(7);
  await rm(join(dir, 'token')); await writeFile(join(dir, 'other'), 'secret', { mode: 0o600 });
  await symlink(join(dir, 'other'), join(dir, 'token')); expect((await run(['status'])).code).toBe(7);
  expect(requests).toHaveLength(0);
}));

test('unsafe server URLs cannot receive credentials', async () => fixture(async ({ run, requests }) => {
  for (const server of ['http://example.org', 'https://user:password@example.org', 'https://example.org/path', 'https://example.org?token=secret']) {
    const result = await run(['whoami', '--server', server]);
    expect(result.code).toBe(2); expect(result.out + result.err).not.toContain('password');
  }
  expect(requests).toHaveLength(0);
}));
