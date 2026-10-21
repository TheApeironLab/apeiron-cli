import { test, expect } from 'bun:test';
import { createHash } from 'node:crypto';
import { mkdtemp, rm, readdir, stat, writeFile, readFile, chmod } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { binary } from './support';

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-sso-'));
  let origin, issuer, refreshes = 0, exchanges = 0, revocations = 0, failRefresh = false, expireRefresh = false, managed = false, iamDenied = false, iamWriteFailure = false;
  const seen = [];
  let lastIssuedRefresh='sso-refresh-0', serverEnv;
  let challenge, deviceReady=true, deviceStarts=0, devicePolls=0, denyDevice=false, slowDevice=false, failDevice=false, failSession=false;
  const server = Bun.serve({ hostname: '127.0.0.1', port: 0, async fetch(request) {
    const path = new URL(request.url).pathname;
    const authorization = request.headers.get('authorization');
    seen.push({ path, authorization, method: request.method, key: request.headers.get("idempotency-key") });
    if (path === '/api/sso/config') return Response.json({ issuer, client_id: 'apeiron-cli', audience: 'apeiron-app', apps: { task: origin + '/task/api' } });
    if (path.endsWith('/.well-known/openid-configuration')) return Response.json({ issuer, token_endpoint: issuer + '/token', device_authorization_endpoint: issuer + '/device', revocation_endpoint: issuer + '/revoke' });
    if (path.endsWith('/device')) {
      deviceStarts++;
      const form = new URLSearchParams(await request.text());
      expect(form.get('client_id')).toBe('apeiron-cli');
      expect(form.has('client_secret')).toBe(false);
      expect(form.get('code_challenge_method')).toBe('S256');
      challenge = form.get('code_challenge');
      expect(challenge).toMatch(/^[A-Za-z0-9_-]{43}$/);
      return Response.json({ device_code: 'fixture-device', user_code: 'TEST-CODE', verification_uri: issuer + '/activate', expires_in: 60, interval: 1 });
    }
    if (path === '/realms/team/token') {
      const form = new URLSearchParams(await request.text());
      expect(form.get('client_id')).toBe('apeiron-cli');
      expect(form.has('client_secret')).toBe(false);
      if (form.get('grant_type') === 'refresh_token') {
        refreshes++;
        if (expireRefresh) return Response.json({error:'invalid_grant'}, {status:400});
        if (failRefresh) return Response.json({ error: 'temporarily_unavailable' }, { status: 503 });
        expect(form.get('refresh_token')).toBe('sso-refresh-0');
        lastIssuedRefresh='sso-refresh-1';
        return Response.json({ access_token: 'sso-access-1', refresh_token: 'sso-refresh-1', expires_in: 300, token_type: 'Bearer' });
      }
      devicePolls++;
      expect(form.get('grant_type')).toBe('urn:ietf:params:oauth:grant-type:device_code');
      expect(createHash('sha256').update(form.get('code_verifier')).digest('base64url')).toBe(challenge);
      if (failDevice) return Response.json({error:'temporarily_unavailable'},{status:503});
      if (denyDevice) return Response.json({error:'access_denied'},{status:400});
      if (slowDevice) {slowDevice=false;return Response.json({error:'slow_down'},{status:400});}
      if (!deviceReady) return Response.json({error:'authorization_pending'},{status:400});
      return Response.json({ access_token: 'sso-access-0', refresh_token: 'sso-refresh-0', expires_in: 1, token_type: 'Bearer' });
    }
    if (path.endsWith('/revoke')) {
      const form = new URLSearchParams(await request.text());
      expect(form.get('token')).toBe(lastIssuedRefresh);
      revocations++;
      return new Response(null, { status: 200 });
    }
    if (path === '/api/sso/session') {
      if (failSession) return Response.json({error:'unavailable'},{status:503});
      expect(['Bearer sso-access-0', 'Bearer sso-access-1']).toContain(authorization);
      return Response.json({ issuer, subject: 'same-user', tenant: 'same-team', client_id: 'apeiron-cli', expires_at: Date.now() + 300000 });
    }
    if (path === '/api/sso/token') {
      expect(authorization).toBe('Bearer sso-access-1');
      expect(await request.json()).toEqual({ app: 'task' });
      exchanges++;
      return Response.json({ app: 'task', issuer, subject: 'same-user', tenant: 'same-team', api_url: origin + '/task/api', access_token: 'task-only-access', expires_at: Date.now() + 300000 });
    }
    if (path.startsWith('/api/organization')) {
      expect(authorization).toBe('Bearer sso-access-1');
      if (iamDenied) return Response.json({error:'denied'}, {status:403});
      if (request.method !== 'GET') {
        const body = await request.json();
        if (iamWriteFailure) return Response.json({error:'private server diagnostic'}, {status:503});
        return Response.json({member:{subject:'member-1',enabled:body.enabled ?? true,apps:{task:body.access ?? 'none'}},credential_written:true});
      }
      if (path === '/api/organization') return Response.json({tenant:{tenant_id:'same-team',name:'Team'},platform_admin:false,apps:[{id:'task',name:'Task',admin:true}]});
      if (path.endsWith('/members')) return Response.json({items:[{subject:'member-1',username:'fixture'}],has_more:false});
      if (path.endsWith('/operations')) return Response.json({items:[]});
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
    if (serverEnv) env.APEIRON_SERVER=serverEnv;
    if (managed) { env.KANEO_API_URL = origin + '/task/api'; env.KANEO_ACCESS_TOKEN_FILE = join(root, 'managed-token'); }
    const child = Bun.spawn([binary, ...args], { env, stdout: 'pipe', stderr: 'pipe' });
    const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
    for (const secret of ['sso-access-', 'sso-refresh-', 'task-only-access','fixture-device']) {
      expect(stdout).not.toContain(secret); expect(stderr).not.toContain(secret);
    }
    return { code, stdout, stderr };
  };
  return { root, origin, run, seen, serverEnv:(value)=>{serverEnv=value;},
    deviceCounts:()=>({starts:deviceStarts,polls:devicePolls}), waitDevice:()=>{deviceReady=false;}, approveDevice:()=>{deviceReady=true;}, denyDevice:()=>{denyDevice=true;}, slowDevice:()=>{slowDevice=true;}, failDevice:()=>{failDevice=true;}, failSession:(value)=>{failSession=value;}, async managed() { await writeFile(join(root, 'managed-token'), 'old-managed-token', {mode:0o600}); managed = true; }, denyIam: () => { iamDenied = true; }, failIamWrite: () => { iamWriteFailure = true; }, counts: () => ({ refreshes, exchanges, revocations }), failRefresh: () => { failRefresh = true; }, expireRefresh: () => { expireRefresh = true; },
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

test('IAM uses the platform SSO audience, preserves mutation idempotency and honors live API denial', async () => {
  const f = await fixture();
  const key = 'ec299fc0-7672-496b-88d2-e194994848bb';
  const file = join(f.root, 'iam.json');
  try {
    expect((await f.run('iam', 'member', 'list', '--json')).code).toBe(3);
    expect((await f.run('sso', 'login', '--server', f.origin, '--no-open', '--json')).code).toBe(0);
    expect(JSON.parse((await f.run('iam', 'organization', 'get', '--json')).stdout).data.tenant.tenant_id).toBe('same-team');
    expect(JSON.parse((await f.run('iam', 'app', 'list', '--json')).stdout).data.items[0].id).toBe('task');
    expect(JSON.parse((await f.run('iam', 'member', 'list', '--offset', '50', '--json')).stdout).data.items[0].subject).toBe('member-1');
    expect(JSON.parse((await f.run('iam', 'operation', 'list', '--json')).stdout).data.items).toEqual([]);
    await writeFile(file, JSON.stringify({app_id:'task',access:'member'}), {mode:0o600});
    const updated = await f.run('iam', 'member', 'update', 'member-1', '--file', file, '--idempotency-key', key, '--json');
    expect(updated.code).toBe(0);
    expect(f.seen.find(v=>v.method==='PATCH')).toMatchObject({path:'/api/organization/members/member-1',key});
    await writeFile(file, JSON.stringify({temporary_password:'Fixture-private-password'}));
    const reset = await f.run('iam', 'member', 'reset-password', 'member-1', '--file', file, '--idempotency-key', key, '--json');
    expect(reset.code).toBe(0); expect(reset.stdout).not.toContain('Fixture-private-password');
    expect(f.seen.find(v=>v.path.endsWith('/password'))).toMatchObject({method:'POST',key});
    expect(f.counts()).toEqual({refreshes:1,exchanges:0,revocations:0});
    expect((await f.run('task', 'whoami', '--json')).code).toBe(0);
    expect(f.counts().exchanges).toBe(1);
    f.failIamWrite();
    const unknown = await f.run('iam', 'member', 'create', '--file', file, '--idempotency-key', key, '--json');
    expect(unknown.code).toBe(9);
    expect(JSON.parse(unknown.stdout).error).toMatchObject({code:'WRITE_OUTCOME_UNKNOWN',outcome:'unknown',httpStatus:503});
    expect(unknown.stdout).not.toContain('private server diagnostic');
    expect(f.seen.filter(v=>v.path==='/api/organization/members'&&v.method==='POST')).toHaveLength(1);
    const count=f.seen.length;
    await writeFile(file,'invalid private input');
    expect((await f.run('iam','member','create','--file',file,'--idempotency-key',key,'--json')).code).toBe(2);
    expect((await f.run('iam','member','list','--api-url','https://foreign.invalid','--json')).code).toBe(2);
    expect(f.seen.length).toBe(count);
    f.denyIam();
    expect((await f.run('iam','member','list','--json')).code).toBe(4);
  } finally { await f.close(); }
}, 30000);


test('expired platform grant directs Task callers back to shared SSO login', async () => {
  const f=await fixture();
  try {
    expect((await f.run('sso','login','--server',f.origin,'--no-open','--json')).code).toBe(0);
    f.expireRefresh();
    const result=await f.run('task','whoami','--json');
    expect(result.code).toBe(3);
    expect(result.stdout).toContain('apeiron sso login');
    expect(JSON.parse(result.stdout).error.code).toBe('REAUTH_REQUIRED');
    expect(result.stdout).not.toContain('task auth login');
    expect((await f.run('task','whoami','--json')).code).toBe(3);
    expect(f.counts().refreshes).toBe(1);
  } finally { await f.close(); }
},15000);


test('headless SSO returns a reusable URL immediately and completes one bounded poll after browser approval', async () => {
  const f=await fixture();f.waitDevice();
  const decode=r=>JSON.parse(r.stdout).data;
  try {
    const start=Date.now();const begun=await f.run('sso','login','--server',f.origin,'--no-open','--no-wait','--json');
    expect(begun.code).toBe(0);expect(Date.now()-start).toBeLessThan(5000);
    expect(decode(begun)).toMatchObject({loggedIn:false,authorizationPending:true,verificationUri:f.origin+'/realms/team/activate',userCode:'TEST-CODE',nextCommand:'apeiron sso login --complete --json'});
    expect(decode(await f.run('sso','login','--server',f.origin,'--no-wait','--json')).authorizationPending).toBe(true);
    expect(f.deviceCounts()).toEqual({starts:1,polls:0});
    expect(decode(await f.run('sso','status','--json')).authorizationPending).toBe(true);
    await Bun.sleep(1100);
    expect(decode(await f.run('sso','login','--complete','--json')).authorizationPending).toBe(true);
    expect(f.deviceCounts().polls).toBe(1);
    const filesBefore=await readdir(join(f.root,'config','apeiron-sso'),{recursive:true});
    const pendingPath=join(f.root,'config','apeiron-sso',filesBefore.find(p=>p.endsWith('pending-login.json')));
    const pending=JSON.parse(await readFile(pendingPath,'utf8'));expect(begun.stdout).not.toContain(pending.device.code_verifier);
    if(process.platform!=='win32')expect((await stat(pendingPath)).mode&0o777).toBe(0o600);
    f.serverEnv('http://127.0.0.1:1');
    f.approveDevice();await Bun.sleep(1100);
    const replies=await Promise.all([f.run('sso','login','--complete','--json'),f.run('sso','login','--complete','--json')]);
    for (const reply of replies) {expect(reply.code).toBe(0);expect(decode(reply).loggedIn).toBe(true);}
    expect(f.deviceCounts()).toEqual({starts:1,polls:2});
    expect(decode(await f.run('task','workspace','list','--json')).items[0].id).toBe('workspace-1');
    const files=await readdir(join(f.root,'config','apeiron-sso'),{recursive:true});expect(files.some(p=>p.endsWith('pending-login.json'))).toBe(false);
  } finally {await f.close();}
},20000);

test('headless device slow_down honors its interval and logout clears a pending authorization', async () => {
 const f=await fixture();f.waitDevice();f.slowDevice();
 try {
  await f.run('sso','login','--server',f.origin,'--no-wait','--json');await Bun.sleep(1100);
  const p=JSON.parse((await f.run('sso','login','--complete','--json')).stdout).data;
  expect(p.authorizationPending).toBe(true);expect(p.retryAfterMs).toBeGreaterThan(5000);
  await f.run('sso','login','--complete','--json');expect(f.deviceCounts().polls).toBe(1);
  expect((await f.run('sso','logout','--json')).code).toBe(0);
  expect(JSON.parse((await f.run('sso','status','--json')).stdout).data.loggedIn).toBe(false);
  expect((await f.run('sso','login','--complete','--json')).code).toBe(3);
 } finally {await f.close();}
},15000);

test('an ambiguous device exchange is not replayed, while acquired tokens survive session-validation failure', async () => {
 for (const ambiguous of [false,true]) {
  const f=await fixture();
  try {
   await f.run('sso','login','--server',f.origin,'--no-wait','--json');await Bun.sleep(1100);
   if (ambiguous) f.failDevice();else f.failSession(true);
   expect((await f.run('sso','login','--complete','--json')).code).not.toBe(0);
   f.failSession(false);
   const next=await f.run('sso','login','--complete','--json');
   expect(next.code).toBe(ambiguous?3:0);expect(f.deviceCounts().polls).toBe(1);
   expect((await f.run('sso','logout','--json')).code).toBe(0);
  } finally {await f.close();}
 }
},15000);


test('denied replacement login preserves the existing session; expired or public pending state is rejected', async () => {
 const f=await fixture();
 try {
  await f.run('sso','login','--server',f.origin,'--no-open','--json');
  const base=join(f.root,'config','apeiron-sso');let files=await readdir(base,{recursive:true});
  const current=join(base,files.find(p=>p.endsWith('.json')&&!p.endsWith('current.json')));const saved=await readFile(current,'utf8');
  f.denyDevice();await f.run('sso','login','--server',f.origin,'--no-wait','--json');
  expect(await readFile(current,'utf8')).toBe(saved);await Bun.sleep(1100);
  const denied=await f.run('sso','login','--complete','--json');expect(denied.code).toBe(3);expect(JSON.parse(denied.stderr).error.code).toBe('REAUTH_REQUIRED');
  expect(await readFile(current,'utf8')).toBe(saved);
  expect((await f.run('task','workspace','list','--json')).code).toBe(0);
  await f.run('sso','login','--server',f.origin,'--no-wait','--json');
  files=await readdir(base,{recursive:true});const path=join(base,files.find(p=>p.endsWith('pending-login.json')));
  if(process.platform!=='win32'){
   await chmod(path,0o644);expect((await f.run('sso','login','--complete','--json')).code).toBe(2);await chmod(path,0o600);
  }
  const pending=JSON.parse(await readFile(path,'utf8'));pending.device.expires_at=1;await writeFile(path,JSON.stringify(pending),{mode:0o600});
  expect((await f.run('sso','login','--complete','--json')).code).toBe(3);
  expect((await readdir(base,{recursive:true})).some(p=>p.endsWith('pending-login.json'))).toBe(false);
 } finally {await f.close();}
},20000);
