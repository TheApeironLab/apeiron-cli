// Tests always execute the Rust CLI. Overrides select a built artifact, never an implementation.
export const binary = process.env.APEIRON_TEST_BIN || new URL('../target/debug/apeiron', import.meta.url).pathname;
export const post = (server, route, body = {}, headers = {}) => fetch(server.url + 'api/' + route, {
  method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(body),
});
export async function until(get, done) {
  for (let n = 0; n < 500; n++) { const value = await get(); if (done(value)) return value; await Bun.sleep(10); }
  throw new Error('Timed out waiting for CLI state');
}
export const localInstallation = () => ({ topology: 'single-k3d', domain: 'example.internal', entryIp: '127.0.0.1', httpPort: 54320, httpsPort: 54321, ha: false, sshUser: '', sshKey: '', sshPort: 22, nodes: [] });

// HTTP test client, not a deployment implementation: every transition is owned by Rust.
export class DeploymentClient {
  constructor(path, root, env = {}) { this.path = path; this.root = root; this.env = env; this.snapshot = { phase: 'idle', message: '' }; this.revision = null; }
  get active() { return ['preparing', 'running', 'stopping'].includes(this.snapshot.phase); }
  async refresh() { this.snapshot = await fetch(this.server.url + 'api/deployment').then(r => r.json()); }
  async start(config) {
    if (!this.server) {
      const { startInitServer } = await import('./rust-server');
      this.server = await startInitServer({ path: this.path, env: { ...this.env, APEIRON_CHENTU_ROOT: this.root } });
      this.timer = setInterval(() => this.refresh().catch(() => {}), 10);
    }
    const response = await post(this.server, 'deploy', { ...config, revision: this.revision });
    const value = await response.json();
    if (!response.ok) throw new Error(value.error);
    this.revision = value.revision;
    await this.refresh();
  }
  async stop() { await post(this.server, 'deployment/stop'); await until(async () => { await this.refresh(); return this.snapshot; }, s => !['preparing', 'running', 'stopping'].includes(s.phase)); }
  async initialAdmin() { const response = await post(this.server, 'credentials'); const value = await response.json(); if (!response.ok) throw new Error(value.error); return value; }
  async close() { clearInterval(this.timer); await this.server?.stop(); }
}
