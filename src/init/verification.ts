import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { open } from 'node:fs/promises';
import { lookup } from 'node:dns/promises';
import { hostname } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { type AccessInfo } from './access';
import { ConfigError, type DeploymentTarget } from './config';
import { safeDomain, safeEntryIp, safeName } from './installation';

export interface InitialAdmin { username: string; password: string }
const credentialError = () => new ConfigError('暂时无法读取初始管理员凭据，请确认集群可连接后重试。');
const execute = async (command: string, args: string[], signal: AbortSignal) => {
  const { stdout } = await promisify(execFile)(command, args, { timeout: 20_000, maxBuffer: 65_536, signal });
  return stdout;
};

export function decodeInitialAdmin(output: string): InitialAdmin {
  try {
    const value = JSON.parse(output);
    if (value.metadata?.name !== 'keycloak-bootstrap' || value.metadata?.namespace !== 'keycloak') throw new Error();
    const decode = (key: string, max: number) => {
      const encoded = value.data?.[key];
      if (typeof encoded !== 'string' || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(encoded) || encoded.length > max * 2) throw new Error();
      const text = new TextDecoder('utf-8', { fatal: true }).decode(Buffer.from(encoded, 'base64'));
      if (!text || text.length > max || /[\x00-\x1f\x7f]/.test(text)) throw new Error();
      return text;
    };
    return { username: decode('username', 256), password: decode('password', 4096) };
  } catch { throw credentialError(); }
}

// Read only the fixed bootstrap Secret, using the kubeconfig produced by this
// successful deployment. stdout/stderr are never forwarded to deployment logs.
export async function readInitialAdmin(target: DeploymentTarget, signal: AbortSignal, run = execute): Promise<InitialAdmin> {
  try {
    const args = ['--request-timeout=8s', '-n', 'keycloak', 'get', 'secret', 'keycloak-bootstrap', '-o', 'json'];
    if (target.runner === 'native') return decodeInitialAdmin(await run('kubectl', ['--kubeconfig', target.kubeconfig, ...args], signal));
    const file = await open(join(target.workDir, 'installation.json'), constants.O_RDONLY | constants.O_NOFOLLOW);
    let cluster: unknown;
    try {
      const info = await file.stat(); if (!info.isFile() || info.size > 8192) throw new Error();
      const marker = JSON.parse(await file.readFile('utf8'));
      if (marker.domain !== target.installation?.domain) throw new Error();
      cluster = marker.cluster;
    } finally { await file.close(); }
    if (!safeName(cluster) || !target.image || !/^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*$/.test(target.image)) throw new Error();
    // No Docker socket or writable mounts. The toolbox is already installed;
    // reading credentials must never download or start another deployment.
    return decodeInitialAdmin(await run('docker', ['run', '--rm', '--pull=never', '--network', `k3d-${cluster}`,
      '--entrypoint', 'kubectl', '-v', `${join(target.workDir, 'state/kubeconfig')}:/kubeconfig:ro`, target.image,
      '--kubeconfig', '/kubeconfig', ...args], signal));
  } catch { throw credentialError(); }
}

export const TEST_ENDPOINTS = [{ id: 'apeiron', name: 'Apeiron' }, { id: 'ops', name: 'Apeiron Ops' }, { id: 'iam', name: 'IAM' }] as const;
export interface VerificationCheck {
  name: string; host: string; url: string;
  dns: 'passed' | 'failed'; https: 'passed' | 'failed' | 'skipped';
  message: string; httpStatus?: number;
}
export interface VerificationResult { checkedFrom: string; checkedAt: string; passed: boolean; checks: VerificationCheck[] }

async function resolveHost(host: string, signal: AbortSignal): Promise<string[]> {
  const deadline = AbortSignal.any([signal, AbortSignal.timeout(4000)]);
  let cancel!: () => void;
  try {
    deadline.throwIfAborted();
    const aborted = new Promise<never>((_, reject) => { cancel = () => reject(new Error('cancelled')); deadline.addEventListener('abort', cancel, { once: true }); });
    return (await Promise.race([lookup(host, { all: true }), aborted])).map(entry => entry.address);
  } finally { if (cancel) deadline.removeEventListener('abort', cancel); }
}
async function checkHttps(host: string, ip: string, signal: AbortSignal, port = 443): Promise<number> {
  const { stdout } = await promisify(execFile)(process.platform === 'darwin' ? '/usr/bin/curl' : 'curl', [
    '--disable', '--silent', '--show-error', '--head', '--noproxy', '*', '--max-time', '10', '--proto', '=https',
    '--resolve', `${host}:${port}:${ip}`, '--output', '/dev/null', '--write-out', '%{http_code}', `https://${host}${port === 443 ? '' : ':' + port}/`,
  ], { signal, timeout: 12_000, maxBuffer: 4096, env: { PATH: process.env.PATH } });
  // Use OS trust, pin the expected entry IP, never follow redirects or send a password.
  if (!/^\d{3}$/.test(stdout.trim())) throw new Error();
  return Number(stdout.trim());
}
export async function verifyInstallation(access: AccessInfo, signal: AbortSignal,
  operations = { resolveHost, checkHttps }): Promise<VerificationResult> {
  if (!safeDomain(access.domain) || !safeEntryIp(access.entryIp)) throw new ConfigError('部署访问地址无效。');
  const port = access.httpsPort ?? 443;
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new ConfigError('HTTPS 端口无效。');
  const checks = await Promise.all(TEST_ENDPOINTS.map(async (site): Promise<VerificationCheck> => {
    const host = `${site.id}.${access.domain}`;
    const base = { name: site.name, host, url: `https://${host}${port === 443 ? '' : ':' + port}/` };
    let addresses: string[];
    try { addresses = await operations.resolveHost(host, signal); }
    catch { return { ...base, dns: 'failed', https: 'skipped', message: '解析失败或超时，请返回配置访问。' } as VerificationCheck; }
    if (!addresses.length || addresses.some(address => address !== access.entryIp)) return { ...base, dns: 'failed', https: 'skipped', message: '解析地址与部署入口不一致，请检查 hosts / DNS。' } as VerificationCheck;
    try {
      const httpStatus = await operations.checkHttps(host, access.entryIp, signal, port);
      const passed = httpStatus >= 200 && httpStatus < 400;
      return { ...base, dns: 'passed', https: passed ? 'passed' : 'failed', httpStatus,
        message: passed ? `HTTPS 可访问 · HTTP ${httpStatus}` : `HTTP ${httpStatus}，请检查应用状态。` } as VerificationCheck;
    } catch { return { ...base, dns: 'passed', https: 'failed', message: 'HTTPS 检查失败，请检查 CA 信任和应用状态。' } as VerificationCheck; }
  }));
  signal.throwIfAborted();
  return { checkedFrom: hostname(), checkedAt: new Date().toISOString(), passed: checks.every(check => check.dns === 'passed' && check.https === 'passed'), checks };
}
