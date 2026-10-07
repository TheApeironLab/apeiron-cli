import { ConfigError, configPath } from './init/config';
import { PairingManager, pairingHelper } from './init/pairing';
import { resolveChentu } from './resources/chentu';

export async function runEntry(args: string[]): Promise<number> {
  const [noun, action, ...flags] = args;
  const help = `schema=apeiron.platform.entry.v1
usage
apeiron platform entry pair --domain team.example.com --public-ip <IPv4> [--ssh-host <host>] [--ssh-port 22]
apeiron platform entry status|revoke --domain team.example.com
apeiron platform connection list [--config <path>]
apeiron platform connection status|test|revoke --id <id> [--config <path>]
Entry commands run on the ECS as root; pairing codes expire after 10 minutes.
Connection commands run on the CLI host. Add --json for machine-readable output.
Exit codes: 0 success, 2 invalid input, 9 operation not confirmed.`;
  if (!action || args.includes('--help') || args.includes('-h')) { console.log(help); return 0; }
  try {
    const options: Record<string, string> = {};
    let json = false;
    const allowed = noun === 'entry' ? action === 'pair' ? ['--domain', '--public-ip', '--ssh-host', '--ssh-port'] : ['--domain'] : ['--id', '--config'];
    for (let i = 0; i < flags.length; i++) {
      const flag = flags[i]!;
      if (flag === '--json' && !json) { json = true; continue; }
      if (!allowed.includes(flag) || options[flag] !== undefined || !flags[i + 1] || flags[i + 1]!.startsWith('--')) throw new ConfigError(`Invalid option: ${flag}`);
      options[flag] = flags[++i]!;
    }
    let result: unknown;
    const signal = AbortSignal.timeout(180_000);
    if (noun === 'entry' && ['pair', 'status', 'revoke'].includes(action)) {
      if (!options['--domain'] || (action === 'pair' && !options['--public-ip'])) throw new ConfigError('--domain and, for pair, --public-ip are required');
      if (process.platform !== 'linux' || process.getuid?.() !== 0) throw new ConfigError('Run entry administration on the Ubuntu ECS with sudo');
      const root = await resolveChentu('', signal, () => {}, { offline: false, bundleDir: '' });
      result = await pairingHelper(root, [action === 'pair' ? 'invite' : action, ...(action === 'pair' ? [] : [options['--domain']!])],
        action === 'pair' ? { domain: options['--domain'], publicIp: options['--public-ip'], host: options['--ssh-host'], sshPort: Number(options['--ssh-port'] ?? 22) } : {}, signal);
    } else if (noun === 'connection' && ['list', 'status', 'test', 'revoke'].includes(action)) {
      const manager = new PairingManager(configPath(options['--config']));
      if (action !== 'list' && !options['--id']) throw new ConfigError('--id is required');
      result = action === 'list' ? await manager.list() : await manager.action(action as 'status' | 'test' | 'revoke', options['--id']!, signal);
    } else throw new ConfigError('Unknown entry command; use apeiron platform entry --help');
    if (json) console.log(JSON.stringify(result));
    else {
      console.log('schema=apeiron.platform.entry.v1');
      if (Array.isArray(result)) { console.log(`count=${result.length}\nid\tdomain\tstate`); for (const row of result) console.log(`${row.id}\t${row.domain}\t${row.state}`); }
      else for (const [key, value] of Object.entries(result as object)) if (['code', 'domain', 'state', 'tunnel', 'routes', 'dns', 'https', 'expiresAt'].includes(key)) console.log(`${key}\t${value}`);
    }
    if (noun === 'connection' && action === 'test' && result && typeof result === 'object') {
      const check = result as { dns?: boolean; https?: boolean; tunnel?: boolean };
      if (!check.dns || !check.https || !check.tunnel) return 9;
    }
    return 0;
  } catch (error) {
    console.error(`schema: apeiron.platform.entry.v1\nerror: ${error instanceof ConfigError ? error.message : 'Entry operation failed; inspect the entry audit log.'}\nretry: apeiron platform entry --help`);
    return error instanceof ConfigError && error.status < 500 ? 2 : 9;
  }
}
