import { test, expect } from 'bun:test';
import { mkdtemp, rm, readdir, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { binary } from './support';

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-sso-'));
  let origin, issuer, refreshes = 0, exchanges = 0, revocations = 0, failRefresh = false, managed = false;
  const seen = [];
  const server = Bun.serve({ hostname: '127.0.0.1', port: 0, async fetch(request) {
    const path = new URL(request.url).pathname;
    const authorization = request.headers.get('authorization');
    seen.push({ path, authorization });
    if (path === '/api/sso/config') return Response.json({ issuer, client_id: 'apeiron-cli', audience: 'apeiron-app', apps: { task: origin + '/task/api' } });
    if (path.endsWith('/.well-known/openid-configuration')) return Response.json({ issuer, token_endpoint: issuer + '/token', device_authorization_endpoint: issuer + '/device', revocation_endpoint: issuer + '/revoke' });
    if (path.endsWith('/device')) {
      const form = new URLSearchParams(await request.text());
      expect(form.get('client_id')).toBe('apeiron-cli');
      expect(form.has('client_secret')).toBe(false);
      return Response.json({ device_code: 'fixture-device', user_code: 'TEST-CODE', verification_uri: issuer + '/activate', expires_in: 60, interval: 1 });
    }
    if (path === '/realms/team/token') {
      const form = new URLSearchParams(await request.text());
      expect(form.get('client_id')).toBe('apeiron-cli');
      expect(form.has('client_secret')).toBe(false);
      if (form.get('grant_type') === 'refresh_token') {
        refreshes++;
        if (failRefresh) return Response.json({ error: 'temporarily_unavailable' }, { status: 503 });
        expect(form.get('refresh_token')).toBe('sso-refresh-0');
        return Response.json({ access_token: 'sso-access-1', refresh_token: 'sso-refresh-1', expires_in: 300, token_type: 'Bearer' });
      }
      expect(form.get('grant_type')).toBe('urn:ietf:params:oauth:grant-type:device_code');
      return Response.json({ access_token: 'sso-access-0', refresh_token: 'sso-refresh-0', expires_in: 1, token_type: 'Bearer' });
    }
    if (path.endsWith('/revoke')) {
      const form = new URLSearchParams(await request.text());
      expect(form.get('token')).toBe('sso-refresh-1');
      revocations++;
      return new Response(null, { status: 200 });
    }
    if (path === '/api/sso/session') {
      expect(['Bearer sso-access-0', 'Bearer sso-access-1']).toContain(authorization);
      return Response.json({ issuer, subject: 'same-user', tenant: 'same-team', client_id: 'apeiron-cli', expires_at: Date.now() + 300000 });
    }
    if (path === '/api/sso/token') {
      expect(authorization).toBe('Bearer sso-access-1');
      expect(await request.json()).toEqual({ app: 'task' });
      exchanges++;
      return Response.json({ app: 'task', issuer, subject: 'same-user', tenant: 'same-team', api_url: origin + '/task/api', access_token: 'task-only-access', expires_at: Date.now() + 300000 });
    }
    if (path === '/task/api/me') {
      if (authorization !== 'Bearer task-only-access') return Response.json({error:'unauthorized'}, {status:401});
      return Response.json({ user: { id: 'task-user', name: 'Alice', email: 'alice@same-team.example.test' },
        auth: { type: 'bearer', clientId: 'broker', expiresAt: new Date(Date.now() + 300000).toISOString() },
        workspaces: [{ id: 'workspace-1', name: 'Team', slug: 'team', role: 'member' }] });
    }
    return new Response('not found', { status: 404 });
  } });
  origin = `http://127.0.0.1:${server.port}`; issuer = origin + '/realms/team';
  const run = async (...args) => {
    const env = { ...process.env, HOME: root, XDG_CONFIG_HOME: join(root, 'config') };
    for (const key of Object.keys(env)) if (key.startsWith('KANEO_') || key === 'APEIRON_SERVER') delete env[key];
    if (managed) { env.KANEO_API_URL = origin + '/task/api'; env.KANEO_ACCESS_TOKEN_FILE = join(root, 'managed-token'); }
    const child = Bun.spawn([binary, ...args], { env, stdout: 'pipe', stderr: 'pipe' });
    const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
    for (const secret of ['sso-access-', 'sso-refresh-', 'task-only-access']) {
      expect(stdout).not.toContain(secret); expect(stderr).not.toContain(secret);
    }
    return { code, stdout, stderr };
  };
  return { root, origin, run, seen, async managed() { await writeFile(join(root, 'managed-token'), 'old-managed-token', {mode:0o600}); managed = true; }, counts: () => ({ refreshes, exchanges, revocations }), failRefresh: () => { failRefresh = true; },
    async close() { server.stop(true); await rm(root, { recursive: true, force: true }); } };
}

test('SSO login once: Task discovers its endpoint, refreshes the source once and exchanges only an App token across concurrent processes', async () => {
  const f = await fixture();
  try {
    const login = await f.run('sso', 'login', '--server', f.origin, '--no-open', '--json');
    expect(login.code).toBe(0);
    expect(JSON.parse(login.stdout).data).toMatchObject({ subject: 'same-user', tenant: 'same-team', loggedIn: true });
    await f.managed();
    const calls = await Promise.all(Array.from({ length: 6 }, () => f.run('task', 'workspace', 'list', '--json')));
    for (const call of calls) { expect(call.code).toBe(0); expect(JSON.parse(call.stdout).data.items[0].id).toBe('workspace-1'); }
    expect(f.seen.some(v => v.authorization === 'Bearer old-managed-token')).toBe(false);
    expect(f.counts()).toEqual({ refreshes: 1, exchanges: 1, revocations: 0 });
    expect((await f.run('sso', 'status', '--json')).code).toBe(0);
    const mismatch = await f.run('task', 'whoami', '--api-url', f.origin + '/other/api', '--json');
    expect(mismatch.code).toBe(2);
    expect(f.seen.some(v => v.path.startsWith('/other'))).toBe(false);
    const cache = join(f.root, 'config', 'apeiron-sso');
    if (process.platform !== 'win32') expect((await stat(cache)).mode & 0o777).toBe(0o700);
    const [profile] = (await readdir(cache)).filter(v => !v.endsWith('.lock'));
    for (const file of await readdir(join(cache, profile))) {
      if (process.platform !== 'win32') expect((await stat(join(cache, profile, file))).mode & 0o777).toBe(0o600);
    }
    expect((await f.run('sso', 'logout', '--json')).code).toBe(0);
    expect(f.counts().revocations).toBe(1);
    expect(JSON.parse((await f.run('sso', 'status', '--json')).stdout).data.loggedIn).toBe(false);
    expect((await f.run('task', 'whoami', '--json')).code).not.toBe(0);
  } finally { await f.close(); }
}, 30000);

test('an ambiguous SSO refresh is never replayed by the next command', async () => {
  const f = await fixture();
  try {
    expect((await f.run('sso', 'login', '--server', f.origin, '--no-open', '--json')).code).toBe(0);
    f.failRefresh();
    expect((await f.run('task', 'whoami', '--json')).code).not.toBe(0);
    expect((await f.run('task', 'whoami', '--json')).code).not.toBe(0);
    expect(f.counts().refreshes).toBe(1);
    expect(f.counts().exchanges).toBe(0);
    expect((await f.run('sso', 'logout', '--json')).code).toBe(0);
  } finally { await f.close(); }
}, 15000);
