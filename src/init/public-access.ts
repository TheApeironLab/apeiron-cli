import { resolve4, resolve6 } from 'node:dns/promises';
import { join } from 'node:path';
import { writeFile } from 'node:fs/promises';
import { ConfigError, type DeploymentTarget } from './config';
import { checkDns } from './dns';
import type { RunCommand } from './bootstrap';

export async function publicAccessPhase(phase: 'check' | 'prepare' | 'finish', target: DeploymentTarget, directory: string, env: NodeJS.ProcessEnv, run: RunCommand, signal?: AbortSignal) {
  const installation = target.installation;
  if (!installation?.publicAccess) return;
  const script = join(target.root, 'bootstrap/public_access.py');
  if (!await Bun.file(script).exists()) throw new ConfigError('此宸途安装包尚未包含 Caddy 公网入口，请使用新版发行包。尚未修改公网入口。');
  if (phase === 'check') {
    // Ignore local /etc/hosts overrides used by internal deployment hooks.
    const result = await checkDns({ domain: installation.domain, entryIp: installation.publicAccess.publicIp, local: false }, {
      signal, includeRoot: true, lookup: async host => (await Promise.all([resolve4(host), resolve6(host).catch(() => [])])).flat().map(address => ({ address })),
    });
    if (!result.passed) throw new ConfigError(`公网 DNS 尚未通过。请将 ${installation.domain} 和 *.${installation.domain} 的 A 记录指向 ${installation.publicAccess.publicIp} 后重试；尚未创建集群。`);
  }
  const plan = join(directory, 'public-access.json');
  await writeFile(plan, JSON.stringify({ topology: installation.topology, domain: installation.domain,
    access: installation.publicAccess, ca: join(target.workDir || directory, 'state/chentu-ca.crt') }), { mode: 0o600 });
  const python = env.CHENTU_PYTHON || Bun.which('python3', { PATH: env.PATH }) || 'python3';
  const args = [script, phase, plan];
  const root = process.getuid?.() === 0;
  const code = await run(root ? python : 'sudo', root ? args : ['-n', 'env', `KUBECONFIG=${target.kubeconfig}`, `PATH=${env.PATH || process.env.PATH}`, ...(env.SSH_AUTH_SOCK ? [`SSH_AUTH_SOCK=${env.SSH_AUTH_SOCK}`] : []), python, ...args], target.root, env);
  if (code !== 0) throw new ConfigError(`公网入口${phase === 'check' ? '检查' : phase === 'prepare' ? '内部网络配置' : '配置或 HTTPS 验证'}失败。请查看日志后重试；已部署的数据保留。`);
}
