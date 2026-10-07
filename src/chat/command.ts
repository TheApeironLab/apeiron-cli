import { constants } from 'node:fs';
import { open } from 'node:fs/promises';
import { parseArgs } from 'node:util';

const HELP = `schema=apeiron.chat.v1
apeiron chat status|whoami
apeiron chat user search --query TEXT [--limit 10]
apeiron chat room list
apeiron chat room create --user @user:server [--name TEXT] [--dry-run]
apeiron chat room join|leave --room ID [--dry-run]
apeiron chat room invite --room ID --user ID [--dry-run]
apeiron chat room read --room ID --event ID [--dry-run]
apeiron chat message send --room ID --text TEXT --txn-id ID [--dry-run]
apeiron chat message list --room ID [--cursor CURSOR] [--limit 10]
apeiron chat message edit --room ID --event ID --text TEXT --txn-id ID [--dry-run]
apeiron chat message redact --room ID --event ID --txn-id ID [--dry-run]
apeiron chat sync --out NEW_FILE [--cursor CURSOR] [--timeout 0]
Options: --server HTTPS_URL --token-file PATH --jsonl --out NEW_FILE
Environment: APEIRON_CHAT_SERVER, APEIRON_CHAT_TOKEN_FILE (Matrix access token, not an Apeiron SSO token)
Output schema: apeiron.chat.v1; default TSV previews truncate text at 160 characters; --out preserves full data.
Exit: 2 invalid input, 4 not found, 5 conflict, 7 authentication/permission, 8 rate limit, 9 upstream/network.
Keep --txn-id unchanged when retrying send/edit/redact. Room creation has no idempotency key: never retry an uncertain result automatically.
Encrypted rooms are unsupported; sending/editing refuses them. No automatic retries or login prompts.`;

class ChatError extends Error {
  constructor(message: string, readonly code = 2, readonly retryAfter?: number) { super(message); }
}
type Row = Record<string, unknown>;
const segment = (value: string) => encodeURIComponent(value);
function object(value: unknown): Row {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ChatError('Invalid Matrix response', 9);
  return value as Row;
}
function items(value: unknown): Row[] {
  if (!Array.isArray(value)) throw new ChatError('Invalid Matrix response', 9);
  return value.map(object);
}
const cell = (value: unknown) => String(value ?? '').replace(/[\x00-\x1f\x7f-\x9f]/g, ' ').slice(0, 160);
async function output(data: Row, jsonl: boolean, path?: string) {
  if (path) {
    let file;
    try {
      file = await open(path, 'wx', 0o600);
      await file.writeFile(JSON.stringify({ schema: 'apeiron.chat.v1', ...data }, null, 2) + '\n');
    } catch { throw new ChatError('Cannot create output file; use a new writable path', 5); }
    finally { await file?.close(); }
    data = { saved: true, path, count: Array.isArray(data.items) ? data.items.length : undefined, next_cursor: data.next_cursor };
  }
  if (jsonl) { console.log(JSON.stringify({ schema: 'apeiron.chat.v1', ...data })); return; }
  console.log('schema=apeiron.chat.v1');
  const rows = Array.isArray(data.items) ? data.items as Row[] : [data];
  if (rows.length) {
    const keys = [...new Set(rows.flatMap(row => Object.keys(row)))];
    console.log(keys.join('\t'));
    for (const row of rows) console.log(keys.map(key => cell(row[key])).join('\t'));
  }
  if (Array.isArray(data.items)) {
    console.log(`count=${rows.length}`);
    if (data.next_cursor !== undefined) console.log(`next_cursor=${JSON.stringify(data.next_cursor)}`);
    if (data.limited !== undefined) console.log(`limited=${data.limited}`);
  }
}

class Matrix {
  constructor(private readonly base: string, private readonly token: string) {}
  async request(method: string, path: string, body?: Row, timeout = 15_000): Promise<Row> {
    let response: Response;
    try {
      response = await fetch(this.base + '/_matrix/client/v3' + path, {
        method, headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: 'error', signal: AbortSignal.timeout(timeout),
      });
    } catch { throw new ChatError('Matrix request failed or timed out; mutation outcome may be unknown', 9); }
    if (!response.ok) {
      const code = response.status === 401 || response.status === 403 ? 7 : response.status === 404 ? 4 :
        response.status === 409 ? 5 : response.status === 429 ? 8 : response.status >= 500 ? 9 : 2;
      // Never echo upstream bodies: authentication failures can contain secrets or HTML.
      const retry = response.headers.get('retry-after');
      throw new ChatError(`Matrix HTTP ${response.status}`, code, retry && /^\d+$/.test(retry) ? Number(retry) : undefined);
    }
    try { return object(await response.json()); }
    catch { throw new ChatError('Invalid Matrix JSON response', 9); }
  }
  async plaintext(room: string) {
    // Read all state so a missing encryption state is distinguishable from an inaccessible room.
    // This is a fail-closed check, not an E2EE implementation. Concurrent encryption changes
    // still require a crypto-aware client; this CLI is only for rooms operated as unencrypted.
    let response: Response;
    try {
      response = await fetch(this.base + '/_matrix/client/v3/rooms/' + segment(room) + '/state', {
        headers: { Authorization: `Bearer ${this.token}` }, redirect: 'error', signal: AbortSignal.timeout(15_000),
      });
    } catch { throw new ChatError('Cannot verify room encryption', 9); }
    if (!response.ok) throw new ChatError('Cannot verify room encryption or membership', response.status === 401 || response.status === 403 ? 7 : 9);
    let state: Row[];
    try { state = items(await response.json()); } catch { throw new ChatError('Invalid room state response', 9); }
    if (state.some(event => event.type === 'm.room.encryption')) throw new ChatError('Encrypted room: use an E2EE-capable client', 2);
  }
}

export async function runChat(args: string[]): Promise<number> {
  try {
    const parsed = parseArgs({ args, allowPositionals: true, strict: true, options: {
      help: { type: 'boolean', short: 'h' }, jsonl: { type: 'boolean' }, 'dry-run': { type: 'boolean' },
      ...Object.fromEntries(['server', 'token-file', 'query', 'limit', 'user', 'name', 'room', 'event', 'text', 'txn-id', 'cursor', 'timeout', 'out'].map(key => [key, { type: 'string' as const }])),
    } });
    const values = parsed.values as Record<string, string | boolean | undefined>;
    const [noun, verb, ...extra] = parsed.positionals;
    if (values.help || !noun) { console.log(HELP); return 0; }
    const action = verb ? `${noun} ${verb}` : noun;
    const commands: Record<string, string[]> = {
      status: [], whoami: [], 'user search': ['query', 'limit'], 'room list': [],
      'room create': ['user', 'name', 'dry-run'], 'room join': ['room', 'dry-run'], 'room leave': ['room', 'dry-run'],
      'room invite': ['room', 'user', 'dry-run'], 'room read': ['room', 'event', 'dry-run'],
      'message send': ['room', 'text', 'txn-id', 'dry-run'], 'message edit': ['room', 'text', 'event', 'txn-id', 'dry-run'],
      'message redact': ['room', 'event', 'txn-id', 'dry-run'], 'message list': ['room', 'cursor', 'limit'], sync: ['cursor', 'timeout'],
    };
    if (extra.length || !commands[action]) throw new ChatError('Unknown chat command; use apeiron chat --help');
    for (const key of Object.keys(values)) if (!['server', 'token-file', 'jsonl', 'out'].includes(key) && !commands[action]!.includes(key)) throw new ChatError(`Unexpected --${key} for ${action}`);
    const str = (key: string, required = true): string => {
      const value = values[key];
      if (typeof value !== 'string' || !value.trim()) {
        if (required) throw new ChatError(`Missing --${key}`);
        return '';
      }
      if (value.length > 65536) throw new ChatError(`--${key} is too long`);
      return value;
    };
    const bounded = (key: string, fallback: number, max: number) => {
      const raw = str(key, false); const value = raw ? Number(raw) : fallback;
      if (!Number.isInteger(value) || value < (key === 'timeout' ? 0 : 1) || value > max) throw new ChatError(`Invalid --${key}`);
      return value;
    };
    // Validate the complete operation before touching credentials or the network.
    let method = 'GET', path = '', body: Row | undefined, room = '', checkEncryption = false;
    let limit = 10;
    const roomPath = () => { room = str('room'); return '/rooms/' + segment(room); };
    switch (action) {
      case 'status': case 'whoami': path = '/account/whoami'; break;
      case 'user search': method = 'POST'; path = '/user_directory/search'; body = { search_term: str('query'), limit: bounded('limit', 10, 100) }; break;
      case 'room list': path = '/joined_rooms'; break;
      case 'room create': method = 'POST'; path = '/createRoom'; body = { preset: 'private_chat', is_direct: true, invite: [str('user')], ...(str('name', false) ? { name: str('name') } : {}) }; break;
      case 'room join': method = 'POST'; path = '/join/' + segment(str('room')); body = {}; break;
      case 'room leave': method = 'POST'; path = roomPath() + '/leave'; body = {}; break;
      case 'room invite': method = 'POST'; path = roomPath() + '/invite'; body = { user_id: str('user') }; break;
      case 'room read': method = 'POST'; path = roomPath() + '/receipt/m.read/' + segment(str('event')); body = {}; break;
      case 'message send': case 'message edit': {
        method = 'PUT'; path = roomPath() + '/send/m.room.message/' + segment(str('txn-id')); checkEncryption = true;
        const content = { msgtype: 'm.text', body: str('text') };
        body = action === 'message send' ? content : { msgtype: 'm.text', body: '* ' + content.body,
          'm.new_content': content, 'm.relates_to': { rel_type: 'm.replace', event_id: str('event') } };
        break;
      }
      case 'message redact': method = 'PUT'; path = roomPath() + '/redact/' + segment(str('event')) + '/' + segment(str('txn-id')); body = {}; break;
      case 'message list': {
        limit = bounded('limit', 10, 100); const query = new URLSearchParams({ dir: 'b', limit: String(limit) });
        if (str('cursor', false)) query.set('from', str('cursor'));
        path = roomPath() + '/messages?' + query; break;
      }
      case 'sync': {
        str('out'); const query = new URLSearchParams({ timeout: String(bounded('timeout', 0, 30) * 1000),
          filter: JSON.stringify({ room: { timeline: { limit: 10 } } }) });
        if (str('cursor', false)) query.set('since', str('cursor'));
        path = '/sync?' + query; break;
      }
    }
    if (values['dry-run']) { await output({ action, method, path, body }, !!values.jsonl, str('out', false)); return 0; }
    const configured = str('server', false) || process.env.APEIRON_CHAT_SERVER || '';
    let url: URL;
    try { url = new URL(configured); } catch { throw new ChatError('Set APEIRON_CHAT_SERVER or --server'); }
    if (url.username || url.password || url.search || url.hash || url.pathname !== '/' ||
        (url.protocol !== 'https:' && !(url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)))) throw new ChatError('Matrix server must be an HTTPS origin (loopback HTTP allowed)');
    const tokenPath = str('token-file', false) || process.env.APEIRON_CHAT_TOKEN_FILE;
    if (!tokenPath) throw new ChatError('Set APEIRON_CHAT_TOKEN_FILE to a Matrix access-token file', 7);
    let token: string;
    let file;
    try {
      file = await open(tokenPath, constants.O_RDONLY | constants.O_NOFOLLOW);
      const stat = await file.stat();
      if (!stat.isFile() || stat.size > 16384 || (process.platform !== 'win32' && ((stat.mode & 0o077) !== 0 || stat.uid !== process.getuid?.()))) throw new Error();
      token = (await file.readFile('utf8')).trim();
      if (!token || /\s/.test(token)) throw new Error();
    } catch { throw new ChatError('Cannot read credential: require an owned regular token file with mode 0600', 7); }
    finally { await file?.close(); }
    const client = new Matrix(url.origin, token);
    if (checkEncryption) await client.plaintext(room);
    const result = await client.request(method, path, body, action === 'sync' ? 45_000 : 15_000);
    const requiredField = ['status', 'whoami'].includes(action) ? 'user_id' : action === 'room create' ? 'room_id' : ['message send', 'message edit', 'message redact'].includes(action) ? 'event_id' : action === 'sync' ? 'next_batch' : undefined;
    if (requiredField && (typeof result[requiredField] !== 'string' || !result[requiredField])) throw new ChatError('Incomplete Matrix response; mutation outcome may be unknown', 9);
    let data: Row = result;
    switch (action) {
      case 'status': case 'whoami': data = { user_id: result.user_id, device_id: result.device_id, connected: true }; break;
      case 'user search': data = { items: items(result.results).map(u => ({ user_id: u.user_id, display_name: u.display_name })), limited: result.limited }; break;
      case 'room list': {
        if (!Array.isArray(result.joined_rooms)) throw new ChatError('Invalid room list', 9);
        if (result.joined_rooms.length > 100 && !values.out) throw new ChatError('Large room list: repeat with --out NEW_FILE', 2);
        data = { items: result.joined_rooms.map(room_id => ({ room_id })) }; break;
      }
      case 'message list': data = { items: items(result.chunk).slice(0, limit).map(e => ({ event_id: e.event_id, sender: e.sender, type: e.type, body: object(e.content).body })), next_cursor: result.end }; break;
      case 'sync': data = { ...result, next_cursor: result.next_batch }; break;
      default: data = { ...result, ok: true };
    }
    await output(data, !!values.jsonl, str('out', false));
    return 0;
  } catch (error) {
    const known = error instanceof ChatError;
    console.error(`schema: apeiron.chat.v1\nerror: ${known ? error.message : 'Invalid command arguments or unexpected response; use apeiron chat --help'}`);
    if (known && error.retryAfter !== undefined) console.error(`retry_after_seconds: ${error.retryAfter}`);
    return known ? error.code : 2;
  }
}
