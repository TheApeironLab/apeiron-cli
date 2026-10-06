import { spawn } from 'node:child_process';
import { ConfigError, configPath } from './config';
import { startInitServer } from './server';

export function initHelp(): string {
  return `schema=apeiron.init.v1
usage: apeiron init [--port <0..65535>] [--config <path>] [--no-open]

Open a local browser wizard for organization, deployment environment and apps.
--port       Loopback port; 0 selects a free port (default).
--config     Local config file; default $XDG_CONFIG_HOME/apeiron/config.json
             or ~/.config/apeiron/config.json. Keep it outside Git repositories.
--no-open    Print the local URL without opening the system browser.

The last step starts Chentu's Helmfile sync and displays deployment status.
Configure paths in the browser. APEIRON_CHENTU_ROOT, CHENTU_ENV, KUBECONFIG,
CHENTU_PROFILE or LAB_ENV, LAB_WORK_DIR, LAB_IMAGE prefill deployment fields.
An existing cluster, environment values, images and deployment tools are required.
Click Finish after deployment or press Ctrl+C to stop the server and active process.
Exit codes: 0 success, 2 invalid input/configuration, 9 deployment/startup failure, 130 cancelled.`;
}

export async function runInit(args: string[]): Promise<number> {
  if (args.includes('--help') || args.includes('-h')) { console.log(initHelp()); return 0; }
  let port = 0;
  let target: string | undefined;
  let openBrowser = true;
  try {
    const seen = new Set<string>();
    for (let i = 0; i < args.length; i++) {
      const flag = args[i]!;
      if (seen.has(flag)) throw new ConfigError(`Duplicate argument: ${flag}`);
      seen.add(flag);
      if (flag === '--no-open') { openBrowser = false; continue; }
      if (flag !== '--port' && flag !== '--config') throw new ConfigError(`Unknown argument: ${flag}`);
      const value = args[++i];
      if (!value || value.startsWith('--')) throw new ConfigError(`Missing value for ${flag}`);
      if (flag === '--config') target = value;
      else {
        if (!/^\d+$/.test(value) || Number(value) > 65535) throw new ConfigError('--port must be an integer from 0 to 65535');
        port = Number(value);
      }
    }
    const path = configPath(target);
    const instance = await startInitServer({ path, port, onSaved: () => console.log('status\tsaved'), onDeployment: status => {
      console.log(`deployment\t${status.phase}\nexit_code\t${status.exitCode ?? ''}`);
      if (status.log) console.log(`log\t${status.log}`);
    } });
    console.log(`schema=apeiron.init.v1\nkey\tvalue\nstatus\tlistening\nurl\t${instance.url}\nconfig\t${path}`);
    const stop = () => { void instance.stop(); };
    process.once('SIGINT', stop);
    process.once('SIGTERM', stop);
    if (openBrowser) {
      const [command, ...prefix] = process.platform === 'darwin' ? ['open'] :
        process.platform === 'win32' ? ['rundll32.exe', 'url.dll,FileProtocolHandler'] : ['xdg-open'];
      const child = spawn(command!, [...prefix, instance.url], { stdio: 'ignore' });
      const warn = () => console.error('browser: Could not open the browser; open the printed local URL manually.');
      child.once('error', warn);
      child.once('exit', code => { if (code && code !== 0) warn(); });
      child.unref();
    }
    await instance.closed;
    process.removeListener('SIGINT', stop);
    process.removeListener('SIGTERM', stop);
    return instance.result.phase === 'failed' ? 9 : instance.result.phase === 'cancelled' ? 130 : 0;
  } catch (error) {
    console.error(`schema: apeiron.init.v1\nerror: ${error instanceof ConfigError ? error.message : 'Could not start the local wizard. Check the config path, permissions and port.'}\nretry: apeiron init --help`);
    return error instanceof ConfigError ? 2 : 9;
  }
}
