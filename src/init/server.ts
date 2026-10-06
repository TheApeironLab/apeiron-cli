import { randomBytes } from 'node:crypto';
import { APPS, ConfigError, ConfigStore, publicSnapshot } from './config';
import { renderPage } from './page';

export async function startInitServer(options: { path: string; port?: number; onSaved?: () => void }) {
  const store = new ConfigStore(options.path);
  await store.checkLocation();
  await store.read(); // Fail before opening a browser if an existing config is unsupported.
  const token = randomBytes(24).toString('hex');
  const base = `/setup/${token}/`;
  const nonce = randomBytes(24).toString('base64');
  const page = renderPage(nonce);
  let origin = '';
  let saved = false;
  let stopping = false;
  let resolveClosed!: () => void;
  const closed = new Promise<void>(resolve => { resolveClosed = resolve; });
  const headers = {
    'Cache-Control': 'no-store',
    'Referrer-Policy': 'no-referrer',
    'X-Content-Type-Options': 'nosniff',
    'Content-Security-Policy': `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'; form-action 'self'`,
  };
  const json = (data: unknown, status = 200) => Response.json(data, { status, headers });
  const server = Bun.serve({
    hostname: '127.0.0.1', port: options.port ?? 0, maxRequestBodySize: 16_384,
    async fetch(request) {
      const url = new URL(request.url);
      if (url.origin !== origin || request.headers.get('host') !== new URL(origin).host ||
          request.headers.get('sec-fetch-site') === 'cross-site') return json({ error: '请求来源不允许。' }, 403);
      if (!url.pathname.startsWith(base) || url.search) return json({ error: '页面不存在。' }, 404);
      if (request.method === 'GET' && url.pathname === base) {
        return new Response(page, { headers: { ...headers, 'Content-Type': 'text/html; charset=utf-8' } });
      }
      try {
        if (url.pathname === base + 'api/config' && request.method === 'GET') {
          return json({ ...publicSnapshot(await store.read()), apps: APPS, path: store.path });
        }
        if (request.method === 'POST') {
          if (request.headers.get('origin') !== origin ||
              request.headers.get('content-type')?.split(';')[0]?.trim() !== 'application/json') {
            return json({ error: '请求来源或格式不允许。' }, 403);
          }
          if (url.pathname === base + 'api/config') {
            let input: unknown;
            try { input = await request.json(); } catch { return json({ error: '请求需为有效 JSON。' }, 400); }
            const snapshot = await store.save(input);
            saved = true;
            options.onSaved?.();
            return json(publicSnapshot(snapshot));
          }
          if (url.pathname === base + 'api/finish') {
            if (!saved) return json({ error: '请先保存配置。' }, 409);
            setTimeout(() => { void stop(); }, 50);
            return json({ ok: true });
          }
        }
        return json({ error: '页面不存在。' }, 404);
      } catch (error) {
        // Filesystem and parser errors can contain file contents; only known,
        // field-level validation messages are allowed back into the browser.
        return json({ error: error instanceof ConfigError ? error.message : '无法读取或保存配置，请检查目录权限后重试。' },
          error instanceof ConfigError ? error.status : 500);
      }
    },
    error() { return json({ error: '请求失败。' }, 500); },
  });
  origin = `http://127.0.0.1:${server.port}`;
  async function stop() {
    if (stopping) return;
    stopping = true;
    await server.stop(false);
    resolveClosed();
  }
  return { url: origin + base, origin, stop, closed };
}
