import { execFile } from 'node:child_process';
import { X509Certificate } from 'node:crypto';
import { constants } from 'node:fs';
import { open, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';
import type { DeploymentTarget } from './config';
import { safeName } from './installation';

export interface AccessInfo {
  domain: string;
  entryIp: string;
  local: boolean;
  public?: boolean;
  httpsPort?: number;
  ca?: { path: string; fingerprint: string; expiresAt: string };
  hostsPath?: string;
  notes: string[];
}
export interface AccessArtifacts {
  info: AccessInfo;
  ca?: string;
  hosts?: string;
}

export function publicCa(pem: string) {
  // Reject bundles, private keys and trailing material; HTTP only receives a
  // canonical public X.509 certificate, never the raw file or a Secret dump.
  if (!/^\s*-----BEGIN CERTIFICATE-----\s+[A-Za-z0-9+/=\s]+-----END CERTIFICATE-----\s*$/.test(pem)) throw new Error('Expected one public certificate');
  const certificate = new X509Certificate(pem);
  if (!certificate.ca || Date.parse(certificate.validFrom) > Date.now() || Date.parse(certificate.validTo) <= Date.now()) throw new Error('Invalid CA');
  return { pem: certificate.toString(), fingerprint: certificate.fingerprint256, expiresAt: new Date(certificate.validTo).toISOString() };
}

export async function collectAccess(target: DeploymentTarget, directory: string, signal: AbortSignal): Promise<AccessArtifacts> {
  const installation = target.installation!;
  if (installation.publicAccess) return { info: { domain: installation.domain, entryIp: installation.publicAccess.publicIp, local: false, public: true, httpsPort: 443, notes: ['公网 DNS 与 HTTPS 已通过检查，证书由 Caddy 自动续期。'] } };
  const result: AccessArtifacts = { info: { domain: installation.domain, entryIp: installation.entryIp,
    local: installation.topology === 'single-k3d', httpsPort: installation.httpsPort, notes: [] } };
  try {
    // Fresh-install values always set work to this directory (mounted at
    // /work/state in the toolbox). Chentu's ingress hook exports the CA here.
    const file = await open(join(target.workDir, 'state/chentu-ca.crt'), constants.O_RDONLY | constants.O_NOFOLLOW);
    let pem: string;
    try {
      const stat = await file.stat();
      if (!stat.isFile() || stat.size > 32_768) throw new Error('Invalid CA file');
      pem = await file.readFile('utf8');
    } finally { await file.close(); }
    const ca = publicCa(pem);
    const path = join(directory, 'chentu-ca.crt');
    await writeFile(path, ca.pem, { mode: 0o600, flag: 'wx' });
    result.ca = ca.pem;
    result.info.ca = { path, fingerprint: ca.fingerprint, expiresAt: ca.expiresAt };
  } catch { result.info.notes.push('尚未读取到有效的公开 CA 证书，请检查宸途 ingress 初始化与本机安装日志。'); }
  try {
    // Keep the hostname inventory owned by Chentu; do not maintain a second
    // application routing table in the CLI. No shell or kubeconfig is involved.
    const { stdout } = await promisify(execFile)('python3', ['-m', 'chentu.lab.ingresshosts', target.root, installation.domain, installation.entryIp], {
      cwd: target.root, env: { ...process.env, PYTHONPATH: join(target.root, 'cli/src'), PYTHONDONTWRITEBYTECODE: '1' },
      timeout: 10_000, maxBuffer: 65_536, signal,
    });
    const names = [...new Set(stdout.trim().split(/\s+/).filter(Boolean).map(pin => {
      const suffix = `.${installation.domain}:${installation.entryIp}`;
      if (!pin.endsWith(suffix) || !safeName(pin.slice(0, -suffix.length))) throw new Error('Unexpected hostname');
      return pin.slice(0, -(installation.entryIp.length + 1));
    }))].sort();
    if (!names.includes(`apeiron.${installation.domain}`) || !names.includes(`iam.${installation.domain}`)) throw new Error('Missing access hostnames');
    result.hosts = `# Apeiron ${installation.domain}\n# Add these entries to this workstation's hosts file. Do not replace the file.\n` +
      names.map(name => `${installation.entryIp} ${name}`).join('\n') + '\n';
    result.info.hostsPath = join(directory, 'apeiron-hosts.txt');
    await writeFile(result.info.hostsPath, result.hosts, { mode: 0o600, flag: 'wx' });
  } catch { result.hosts = undefined; delete result.info.hostsPath; result.info.notes.push('完整 hosts 清单未生成，请使用内网 DNS 泛解析或检查发行包中的主机名清单工具。'); }
  return result;
}
