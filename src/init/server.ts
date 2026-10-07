import { randomBytes } from 'node:crypto';
import { APPS, ConfigError, ConfigStore, deploymentDefaults, publicSnapshot, validateConfig } from './config';
import { Deployment, type DeploymentStatus } from './deploy';
import { renderPage } from './page';
import type { ResourceResolver } from '../resources/chentu';
import { EnvironmentProbe } from './probe';
import { preflightDeployment } from './local-cluster';
import { probeNodes, sshAliases } from './nodes';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { checkDns, cliHost } from './dns';
import { openDeploymentLog } from './log';
import { LocalAccessInstaller } from './local-access';
import { readInitialAdmin, verifyInstallation, type VerificationResult } from './verification';
import faviconPath from './assets/apeiron-favicon.ico' with { type: 'file' };
import { PairingManager } from './pairing';

// Original browser icon from TheApeironLab/apeiron: frontend/fe-apeiron-app/public/favicon.ico.
const favicon = await Bun.file(faviconPath).arrayBuffer();

export async function startInitServer(options: { path: string; port?: number; onSaved?: () => void; onDeployment?: (status: DeploymentStatus) => void; resources?: ResourceResolver; probe?: EnvironmentProbe; localAccess?: LocalAccessInstaller; adminReader?: typeof readInitialAdmin; verify?: typeof verifyInstallation }) {
  const store = new ConfigStore(options.path);
  await store.checkLocation();
  await store.read(); // Fail before opening a browser if an existing config is unsupported.
  const deployment = new Deployment(options.path, options.onDeployment, options.resources, options.adminReader);
  const probe = options.probe ?? new EnvironmentProbe();
  const localAccess = options.localAccess ?? new LocalAccessInstaller();
  const pairing = new PairingManager(options.path, options.resources);
  let pairingBusy = false;
  const nodeController = new AbortController();
  let checkingDeployment = false;
  let scanning = false;
  let picking = false;
  let checkingDns = false;
  let readingAdmin = false;
  let verifying = false;
  let verification: VerificationResult | null = null;
  let deployedRevision: string | null = null;
  const token = randomBytes(24).toString('hex');
  const base = `/setup/${token}/`;
  const nonce = randomBytes(24).toString('base64');
  const page = renderPage(nonce);
  let origin = '';
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
    hostname: '127.0.0.1', port: options.port ?? 0, maxRequestBodySize: 16_384, idleTimeout: 255,
    async fetch(request) {
      const url = new URL(request.url);
      if (url.origin !== origin || request.headers.get('host') !== new URL(origin).host ||
          request.headers.get('sec-fetch-site') === 'cross-site') return json({ error: '请求来源不允许。' }, 403);
      if (!url.pathname.startsWith(base) || url.search) return json({ error: '页面不存在。' }, 404);
      if (request.method === 'GET' && url.pathname === base) {
        return new Response(page, { headers: { ...headers, 'Content-Type': 'text/html; charset=utf-8' } });
      }
      if (request.method === 'GET' && url.pathname === base + 'favicon.ico') {
        return new Response(favicon, { headers: { ...headers, 'Content-Type': 'image/vnd.microsoft.icon' } });
      }
      try {
        if (url.pathname === base + 'api/config' && request.method === 'GET') {
          return json({ ...publicSnapshot(await store.read()), connections: await pairing.list(), apps: APPS, path: store.path, host: cliHost(), defaults: deploymentDefaults(), deployment: deployment.snapshot });
        }
        if (url.pathname === base + 'api/deployment' && request.method === 'GET') {
          return json(deployment.snapshot);
        }
        if (url.pathname === base + 'api/access' && request.method === 'GET') {
          return json({ capability: await localAccess.capability(), status: localAccess.snapshot });
        }
        if (url.pathname === base + 'api/verification' && request.method === 'GET') return json({ result: verification });
        if (request.method === 'GET' && (url.pathname === base + 'api/log' || url.pathname === base + 'api/log/download')) {
          if (request.headers.has('origin') && request.headers.get('origin') !== origin) return json({ error: '请求来源不允许。' }, 403);
          const log = await openDeploymentLog(deployment.snapshot.log);
          return new Response(log.stream, { headers: { ...headers, 'Content-Type': 'text/plain; charset=utf-8',
            'Content-Length': String(log.size),
            'Content-Disposition': `${url.pathname.endsWith('/download') ? 'attachment' : 'inline'}; filename="install.log"` } });
        }
        if (url.pathname === base + 'api/ssh-aliases' && request.method === 'GET') return json({ aliases: await sshAliases() });
        if (request.method === 'GET' && (url.pathname === base + 'api/ca.crt' || url.pathname === base + 'api/hosts.txt')) {
          const ca = url.pathname.endsWith('/ca.crt');
          const content = deployment.download(ca ? 'ca' : 'hosts');
          if (!content) return json({ error: '部署完成并生成文件后才可下载。' }, 404);
          return new Response(content, { headers: { ...headers, 'Content-Type': ca ? 'application/x-pem-file' : 'text/plain; charset=utf-8',
            'Content-Disposition': `attachment; filename="${ca ? 'chentu-ca.crt' : 'apeiron-hosts.txt'}"` } });
        }
        if (request.method === 'POST') {
          if (request.headers.get('origin') !== origin ||
              request.headers.get('content-type')?.split(';')[0]?.trim() !== 'application/json') {
            return json({ error: '请求来源或格式不允许。' }, 403);
          }
          if (url.pathname.startsWith(base + 'api/connections/')) {
            if (pairingBusy || stopping || checkingDeployment || deployment.active || localAccess.active || verifying) return json({ error: '连接管理、部署或测试正在进行，请稍后重试。' }, 409);
            pairingBusy = true;
            try {
              const input = await request.json();
              if (!input || typeof input !== 'object' || Array.isArray(input) || Object.keys(input).length !== 1) throw new ConfigError('连接请求格式不正确。');
              const action = url.pathname.slice((base + 'api/connections/').length);
              const signal = AbortSignal.any([nodeController.signal, request.signal]);
              let connection;
              if (action === 'pair' && typeof input.code === 'string') connection = await pairing.pair(input.code, signal);
              else if (['status', 'test', 'revoke'].includes(action) && typeof input.id === 'string') connection = await pairing.action(action as 'status' | 'test' | 'revoke', input.id, signal);
              else throw new ConfigError('连接操作不支持。');
              return json({ connection, connections: await pairing.list() });
            } finally { pairingBusy = false; }
          }
          if (pairingBusy && ['api/config', 'api/deploy', 'api/deployment/retry', 'api/finish'].some(path => url.pathname === base + path)) return json({ error: '连接管理正在进行，请稍后重试。' }, 409);
          if (url.pathname === base + 'api/deployment/stop' || url.pathname === base + 'api/deployment/retry') {
            if (stopping || checkingDeployment || localAccess.active || readingAdmin || verifying) return json({ error: '向导正在关闭、配置或测试，请稍后重试。' }, 409);
            let input: unknown;
            try { input = await request.json(); } catch { throw new ConfigError('请求需为有效 JSON。'); }
            if (!input || typeof input !== 'object' || Array.isArray(input)) throw new ConfigError('请求格式不正确。');
            if (url.pathname.endsWith('/stop')) {
              if (Object.keys(input).length) throw new ConfigError('停止请求不接受命令或路径参数。');
              if (!deployment.active) return json({ error: '当前没有正在进行的部署。' }, 409);
              void deployment.stop();
              return json({ deployment: deployment.snapshot }, 202);
            }
            if (Object.keys(input).length !== 1 || !('revision' in input) || typeof input.revision !== 'string') throw new ConfigError('重新部署需要当前配置版本。');
            if (checkingDeployment || deployment.active || !['failed', 'cancelled'].includes(deployment.snapshot.phase)) return json({ error: '等待部署完全停止或失败后，才能重新部署。' }, 409);
            const revision = input.revision;
            checkingDeployment = true;
            try {
              const saved = await store.read();
              if (!saved.config || revision !== deployedRevision || revision !== saved.revision) throw new ConfigError('配置已变化，请返回修改或刷新后确认配置，再开始部署。', 409);
              const preflight = await preflightDeployment(saved.config, store.path, nodeController.signal);
              await deployment.start(async () => {
                const current = await store.read();
                if (!current.config || current.revision !== revision) throw new ConfigError('配置已变化，请刷新后重试。', 409);
                return current.config;
              }, preflight?.message);
              localAccess.reset(); verification = null;
            } finally { checkingDeployment = false; }
            return json({ deployment: deployment.snapshot }, 202);
          }
          if (url.pathname === base + 'api/access/install') {
            if (stopping || checkingDeployment || deployment.snapshot.phase !== 'succeeded' || !deployment.snapshot.access) return json({ error: '部署完成后才能配置本机访问。' }, 409);
            let input: unknown;
            try { input = await request.json(); } catch { throw new ConfigError('请求需为有效 JSON。'); }
            // Only the exact server-owned deployment artifacts can reach root.
            if (!input || typeof input !== 'object' || Array.isArray(input) || Object.keys(input).length) throw new ConfigError('安装请求不接受域名、路径或证书参数。');
            await localAccess.start({ info: deployment.snapshot.access, ca: deployment.download('ca'), hosts: deployment.download('hosts') });
            return json({ status: localAccess.snapshot }, 202);
          }
          if (url.pathname === base + 'api/credentials' || url.pathname === base + 'api/verification') {
            if (stopping || checkingDeployment || deployment.snapshot.phase !== 'succeeded' || !deployment.snapshot.access) return json({ error: '部署完成后才能进入测试。' }, 409);
            let input: unknown;
            try { input = await request.json(); } catch { throw new ConfigError('请求需为有效 JSON。'); }
            if (!input || typeof input !== 'object' || Array.isArray(input) || Object.keys(input).length) throw new ConfigError('测试请求不接受路径、域名或凭据参数。');
            const signal = AbortSignal.any([nodeController.signal, request.signal]);
            if (url.pathname.endsWith('/credentials')) {
              if (readingAdmin) return json({ error: '正在读取凭据，请稍后重试。' }, 409);
              readingAdmin = true;
              try { return json(await deployment.initialAdmin(signal)); }
              finally { readingAdmin = false; }
            }
            if (verifying || localAccess.active) return json({ error: '配置或测试正在进行，请稍后重试。' }, 409);
            verifying = true;
            try {
              verification = await (options.verify ?? verifyInstallation)(deployment.snapshot.access, signal);
              return json({ result: verification });
            } finally { verifying = false; }
          }
          if (url.pathname === base + 'api/probe') {
            if (stopping) return json({ error: '向导正在关闭。' }, 409);
            let input: unknown;
            try { input = await request.json(); } catch { return json({ error: '请求需为有效 JSON。' }, 400); }
            if (!input || typeof input !== 'object' || Array.isArray(input) || !('offline' in input) || typeof input.offline !== 'boolean' ||
                ('refresh' in input && typeof input.refresh !== 'boolean')) return json({ error: '探测需要明确指定在线或离线模式。' }, 400);
            return json(await probe.run(input.offline, 'refresh' in input && input.refresh === true));
          }
          if (url.pathname === base + 'api/dns') {
            if (checkingDns || stopping) return json({ error: '解析检测正在进行，请稍后重试。' }, 409);
            let input: unknown;
            try { input = await request.json(); } catch { throw new ConfigError('请求需为有效 JSON。'); }
            checkingDns = true;
            try { return json(await checkDns(input, { signal: AbortSignal.any([nodeController.signal, request.signal]) })); }
            finally { checkingDns = false; }
          }
          if (url.pathname === base + 'api/nodes') {
            if (scanning || stopping || deployment.active) return json({ error: '检测或部署正在进行，请稍后重试。' }, 409);
            scanning = true;
            try {
              return json({ nodes: await probeNodes(await request.json(), AbortSignal.any([nodeController.signal, request.signal])) });
            } finally { scanning = false; }
          }
          if (url.pathname === base + 'api/pick-bundle') {
            if (picking || stopping || deployment.active) return json({ error: '目录选择正在进行。' }, 409);
            if (process.platform !== 'darwin') throw new ConfigError('远程或无桌面环境请直接填写 CLI 主机上的绝对路径。');
            picking = true;
            try {
              const result = await promisify(execFile)('osascript', ['-e', 'POSIX path of (choose folder with prompt "选择离线安装包目录")'], { timeout: 120_000, maxBuffer: 8192, signal: nodeController.signal });
              return json({ path: result.stdout.trim() });
            } catch { return json({ path: '' }); }
            finally { picking = false; }
          }
          if (url.pathname === base + 'api/config' || url.pathname === base + 'api/deploy') {
            if (stopping || checkingDeployment || deployment.active || localAccess.active || readingAdmin || verifying) return json({ error: '部署、配置或测试正在进行，暂时不能修改配置或再次启动。' }, 409);
            let input: unknown;
            try { input = await request.json(); } catch { return json({ error: '请求需为有效 JSON。' }, 400); }
            // Parsing the request yields; reserve preflight only after rechecking.
            if (stopping || pairingBusy || checkingDeployment || deployment.active || localAccess.active || readingAdmin || verifying) return json({ error: '部署、配置或测试正在进行，请稍后重试。' }, 409);
            if (url.pathname === base + 'api/deploy') {
              checkingDeployment = true;
              try {
                const config = validateConfig(input);
                const preflight = await preflightDeployment(config, store.path, nodeController.signal);
                await deployment.start(async () => {
                  const saved = await store.save(input);
                  deployedRevision = saved.revision;
                  options.onSaved?.();
                  return saved.config!;
                }, preflight?.message);
                localAccess.reset(); verification = null;
              } finally { checkingDeployment = false; }
              return json({ ...publicSnapshot(await store.read()), deployment: deployment.snapshot }, 202);
            }
            const snapshot = await store.save(input);
            options.onSaved?.();
            return json(publicSnapshot(snapshot));
          }
          if (url.pathname === base + 'api/finish') {
            if (checkingDeployment || deployment.active || localAccess.active || readingAdmin || verifying || deployment.snapshot.phase === 'idle') return json({ error: '部署、配置或测试尚未结束。' }, 409);
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
    probe.cancel();
    nodeController.abort();
    await localAccess.wait();
    await deployment.stop();
    await server.stop(false);
    resolveClosed();
  }
  return { url: origin + base, origin, stop, closed, get result() { return deployment.snapshot; } };
}
