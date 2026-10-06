import { randomBytes } from 'node:crypto';
import { lookup } from 'node:dns/promises';
import { hostname, networkInterfaces } from 'node:os';
import { ConfigError } from './config';
import { safeDomain, safeEntryIp } from './installation';

export function cliHost() {
  const addresses = [...new Set(Object.entries(networkInterfaces()).flatMap(([name, entries]) =>
    /^(lo|utun|docker|veth|br-)/.test(name) ? [] : (entries ?? []).filter(entry =>
      entry.family === 'IPv4' && !entry.internal && safeEntryIp(entry.address)).map(entry => entry.address)))];
  return { name: hostname(), addresses };
}

export interface DnsCheck {
  host: string;
  addresses: string[];
  status: 'matched' | 'mismatch' | 'unresolved' | 'timeout';
}
export interface DnsResult {
  checkedFrom: string;
  checkedAt: string;
  passed: boolean;
  wildcard: boolean;
  checks: DnsCheck[];
}
type Lookup = (host: string) => Promise<{ address: string }[]>;

export async function checkDns(value: unknown, options: { lookup?: Lookup; signal?: AbortSignal; timeoutMs?: number } = {}): Promise<DnsResult> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ConfigError('请填写域名和入口 IP。');
  const input = value as Record<string, unknown>;
  if (!safeDomain(input.domain) || !safeEntryIp(input.entryIp) || typeof input.local !== 'boolean') throw new ConfigError('请填写有效的域名和入口 IPv4 地址。');
  if (input.local && input.entryIp !== '127.0.0.1') throw new ConfigError('本机测试入口必须为 127.0.0.1。');
  const resolve = options.lookup ?? (host => lookup(host, { all: true })); // OS resolver includes hosts and macOS scoped resolvers.
  const hosts = [`apeiron.${input.domain}`, `iam.${input.domain}`,
    ...(!input.local ? [`apeiron-check-${randomBytes(6).toString('hex')}.${input.domain}`] : [])];
  const checks = await Promise.all(hosts.map(async (host): Promise<DnsCheck> => {
    const signal = AbortSignal.any([AbortSignal.timeout(options.timeoutMs ?? 4000), ...(options.signal ? [options.signal] : [])]);
    let cancel!: () => void;
    try {
      signal.throwIfAborted();
      const cancelled = new Promise<never>((_, reject) => { cancel = () => reject(new Error('cancelled')); signal.addEventListener('abort', cancel, { once: true }); });
      const result = await Promise.race([resolve(host), cancelled]);
      const addresses = [...new Set(result.map(item => item.address))];
      return { host, addresses, status: addresses.length && addresses.every(address => address === input.entryIp) ? 'matched' : 'mismatch' };
    } catch {
      return { host, addresses: [], status: signal.aborted ? 'timeout' : 'unresolved' };
    } finally { if (cancel) signal.removeEventListener('abort', cancel); }
  }));
  return { checkedFrom: hostname(), checkedAt: new Date().toISOString(), passed: checks.every(check => check.status === 'matched'), wildcard: !input.local, checks };
}
