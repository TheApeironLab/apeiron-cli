import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { deploymentFixture, freshInstallFixture } from '../test/fixtures';
import type { ProbeResult } from '../src/init/probe';
import type { LocalAccessStatus } from '../src/init/local-access';
import type { VerificationResult } from '../src/init/verification';

const dir = await mkdtemp(join(tmpdir(), 'apeiron-wizard-ui-'));
const path = join(dir, 'config.json');
const setup = await deploymentFixture(dir);
await freshInstallFixture(dir, setup, 17, 'example.internal');
const binary = process.argv[2] ? resolve(process.argv[2]) : resolve(import.meta.dir, '../dist/apeiron');
const child = Bun.spawn([binary, 'init', '--no-open', '--config', path], {
  cwd: dir, stdout: 'pipe', stderr: 'pipe', env: { ...process.env, PATH: setup.bin + ':' + process.env.PATH, LAB_ENV: setup.environment, APEIRON_CHENTU_ROOT: setup.root },
});
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
let startup = '';
const timeout = setTimeout(() => { child.kill(); throw new Error('Browser verification timed out'); }, 60_000);
try {
  const reader = child.stdout.getReader();
  let url = '';
  try {
    while (!url) {
      const result = await reader.read();
      if (result.done) throw new Error('Wizard exited before printing its URL: ' + await new Response(child.stderr).text());
      startup += new TextDecoder().decode(result.value);
      url = /^url\t(.+)$/m.exec(startup)?.[1] ?? '';
    }
  } finally { reader.releaseLock(); }
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 920 } });
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const externalRequests: string[] = [];
  page.on('request', request => { if (new URL(request.url()).origin !== new URL(url).origin) externalRequests.push(request.url()); });
  const probeModes: boolean[] = [];
  let probeReply: 'normal' | 'slow' | 'fail' = 'normal';
  let releaseSlowProbe: (() => void) | undefined;
  const detected: ProbeResult = {
    machine: { os: { name: 'Ubuntu', version: '22.04', kernel: '6.8.0-fixture' },
      hardware: { architecture: 'amd64', runtimeArchitecture: 'amd64', cpu: 'Fixture CPU', cores: 8, memoryGiB: 16 } },
    checkedAt: new Date().toISOString(),
    network: { status: 'limited', checks: [
      { name: '公共网站', host: 'www.microsoft.com', status: 'reachable', elapsedMs: 40, httpStatus: 200 },
      { name: '海外访问（Google）', host: 'www.google.com', status: 'timeout', elapsedMs: 4000 },
      { name: 'GitHub API', host: 'api.github.com', status: 'reachable', elapsedMs: 60, httpStatus: 200 },
      { name: '安装包下载域名', host: 'release-assets.githubusercontent.com', status: 'reachable', elapsedMs: 50, httpStatus: 404 },
    ] },
  };
  // Keep browser verification offline and deterministic. Backend transport and
  // cancellation behavior are exercised through EnvironmentProbe in probe.test.ts.
  await page.route('**/api/probe', async route => {
    const request = route.request().postDataJSON() as { offline: boolean };
    probeModes.push(request.offline);
    const reply = probeReply;
    if (reply === 'slow' && !request.offline) await new Promise<void>(resolve => { releaseSlowProbe = resolve; });
    const result = { ...detected, network: request.offline ? { status: 'skipped', checks: [] } : detected.network };
    await route.fulfill({ status: reply === 'fail' ? 503 : 200, contentType: 'application/json', body: JSON.stringify(result) }).catch(() => {});
  });
  let dnsPassed = false;
  const dnsRequests: { domain: string; entryIp: string; local: boolean }[] = [];
  await page.route('**/api/dns', async route => {
    const input = route.request().postDataJSON();
    dnsRequests.push(input);
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ checkedFrom: 'fixture-mac', checkedAt: new Date().toISOString(), passed: dnsPassed,
      wildcard: !input.local, checks: ['apeiron', 'iam'].map(name => ({ host: `${name}.${input.domain}`, addresses: dnsPassed ? [input.entryIp] : [], status: dnsPassed ? 'matched' : 'unresolved' })) }) });
  });
  // Never open a system authorization prompt during browser verification.
  let accessStatus: LocalAccessStatus = { phase: 'idle', message: '' };
  let desktopAvailable = true;
  let accessRequests = 0;
  await page.route('**/api/access', async route => {
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify({
      capability: { available: desktopAvailable, host: 'fixture-mac', reason: '远程主机没有 Mac 桌面，请使用手动配置。' }, status: accessStatus,
    }) });
  });
  await page.route('**/api/access/install', async route => {
    assert.deepEqual(route.request().postDataJSON(), {}, 'No browser-supplied file paths, commands or certificates');
    accessRequests++;
    accessStatus = { phase: 'installing', message: '等待 macOS 系统授权。' };
    await route.fulfill({ status: 202, contentType: 'application/json', body: JSON.stringify({ status: accessStatus }) });
  });
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { (window as any).__copied = text; } } });
  });
  let verificationPassed = false;
  let verificationResult: VerificationResult | null = null;
  await page.route('**/api/verification', async route => {
    if (route.request().method() === 'POST') {
      assert.deepEqual(route.request().postDataJSON(), {});
      verificationResult = { checkedFrom: 'fixture-mac', checkedAt: new Date().toISOString(), passed: verificationPassed,
        checks: ['apeiron', 'ops', 'iam'].map(id => ({ name: id, host: id + '.example.internal', url: 'https://' + id + '.example.internal/', dns: 'passed', https: verificationPassed ? 'passed' : 'failed', httpStatus: verificationPassed ? 200 : 404, message: verificationPassed ? 'HTTPS 可访问 · HTTP 200' : 'HTTP 404，请检查应用状态。' })) };
    }
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ result: verificationResult }) });
  });
  await page.goto(url);
  await page.getByText('Ubuntu 22.04', { exact: true }).waitFor();
  assert.equal(await page.getByText('AMD64', { exact: true }).isVisible(), true);
  assert.equal(await page.locator('#probe-cores').innerText(), '8 核');
  assert.equal(await page.locator('#probe-memory').innerText(), '16 GiB');
  assert.equal(await page.locator('#system-probe #probe-cpu').isVisible(), true);
  await page.getByText('部分探测点可连接', { exact: true }).waitFor();
  assert.deepEqual(await page.locator('.network-card strong').allTextContents(), ['Microsoft', 'GitHub', 'Google', '下载源']);
  assert.match(await page.locator('[data-host="www.google.com"]').innerText(), /连接超时/);
  assert.match(await page.locator('[data-host="release-assets.githubusercontent.com"]').innerText(), /域名可达/);
  assert.equal(await page.locator('#probe-details').evaluate((node: HTMLDetailsElement) => node.open), false, 'Technical details stay collapsed even when a site times out');
  assert.equal(await page.locator('#probe-sites').isVisible(), false);
  await page.screenshot({ path: join(dir, 'network-cards.png'), fullPage: true });
  await page.getByText('检测详情', { exact: true }).click();
  assert.equal(await page.locator('#probe-sites li').count(), 4);
  assert.match(await page.locator('#probe-sites li').filter({ hasText: 'www.google.com' }).innerText(), /连接超时/);
  const downloadCheck = page.locator('#probe-sites li').filter({ hasText: 'release-assets.githubusercontent.com' });
  assert.match(await downloadCheck.innerText(), /已连接 · HTTP 404/);
  assert.match(await downloadCheck.innerText(), /根路径没有资源。尚未验证具体安装包能否下载/);
  assert.equal(await downloadCheck.locator('[data-state=limited]').count(), 1);
  await page.screenshot({ path: join(dir, 'network-details.png'), fullPage: true });
  await page.getByText('检测详情', { exact: true }).click();
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.screenshot({ path: join(dir, 'mobile-network-cards.png'), fullPage: true });
  await page.setViewportSize({ width: 1280, height: 920 });
  const offline = page.locator('#offline');
  const online = page.locator('#online');
  assert.equal(await offline.isChecked(), false, 'A failed probe does not silently change deployment mode');
  assert.deepEqual(probeModes, [false]);
  probeReply = 'slow';
  await page.getByRole('button', { name: '重新检测', exact: true }).click();
  while (!releaseSlowProbe) await Bun.sleep(10);
  await offline.check();
  await page.getByText('离线模式 · 已跳过', { exact: true }).waitFor();
  await page.getByRole('button', { name: '重新检测', exact: true }).waitFor();
  releaseSlowProbe();
  assert.equal(await page.getByText('离线模式 · 已跳过', { exact: true }).isVisible(), true);
  assert.equal(await page.getByText('Ubuntu 22.04', { exact: true }).isVisible(), true);
  assert.deepEqual(await page.locator('.network-status').allTextContents(), ['已跳过', '已跳过', '已跳过', '已跳过']);
  assert.equal(probeModes.at(-1), true);
  await page.screenshot({ path: join(dir, 'offline-probe.png'), fullPage: true });
  probeReply = 'fail';
  await online.check();
  await page.getByText('检测未完成', { exact: true }).waitFor();
  assert.deepEqual(await page.locator('.network-status').allTextContents(), ['检测失败', '检测失败', '检测失败', '检测失败']);
  probeReply = 'normal';
  await page.getByRole('button', { name: '重新检测', exact: true }).click();
  await page.getByText('部分探测点可连接', { exact: true }).waitFor();
  assert.equal(await page.getByLabel('Base URL', { exact: true }).count(), 0);
  const slug = page.getByLabel('Slug name', { exact: true });
  assert.equal(await slug.isVisible(), false, 'Organization belongs to step 2');
  assert.equal(await page.locator('#environment').count(), 0, 'Fresh installs do not ask for an existing values file');
  assert.equal(await page.locator('#kubeconfig').count(), 0, 'Kubeconfig is generated');
  assert.equal(await page.getByRole('heading', { name: '部署选项', exact: true }).isVisible(), true);
  assert.equal(await page.locator('#node-section').isVisible(), false);
  await page.getByRole('radio', { name: /多机 K3s/ }).check();
  assert.equal(await page.locator('#node-section').isVisible(), true);
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await page.getByRole('heading', { name: '设置部署环境', exact: true }).isVisible(), true);
  const node = (n: number) => ({ host: 'node-' + n, name: 'node-' + n, os: 'ubuntu', version: '22.04', architecture: 'x86_64', cores: 8, memoryGiB: 32, diskGiB: 120, addresses: ['192.0.2.' + (10 + n)], sudo: true, existingCluster: false, supported: true });
  await page.route('**/api/nodes', async route => {
    const input = route.request().postDataJSON() as { hosts: string[] };
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ nodes: input.hosts.map((_, i) => node(i)) }) });
  });
  await page.getByLabel('IP 地址或 SSH 别名', { exact: true }).fill('node-0\nnode-1');
  await page.getByRole('button', { name: '自动检测节点', exact: true }).click();
  await page.getByText('已生成节点配置，请确认内网 IP 和控制／工作角色。部署前会再次核验。', { exact: true }).waitFor();
  assert.deepEqual(await page.locator('[data-field=role]').evaluateAll((nodes: HTMLSelectElement[]) => nodes.map(node => node.value)), ['server', 'agent']);
  assert.equal(await page.locator('[data-panel]').nth(0).locator('#entry-ip').inputValue(), '192.0.2.10');
  await page.getByRole('checkbox', { name: '控制平面高可用', exact: true }).check();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByText('高可用需要至少 3 个、且为奇数个控制节点。', { exact: true }).waitFor();
  await page.getByLabel('IP 地址或 SSH 别名', { exact: true }).fill('node-0\nnode-1\nnode-2');
  assert.equal(await page.locator('.node-result').count(), 0, 'Changing addresses invalidates old probe results');
  await page.getByRole('button', { name: '自动检测节点', exact: true }).click();
  await page.getByText('已生成节点配置，请确认内网 IP 和控制／工作角色。部署前会再次核验。', { exact: true }).waitFor();
  assert.deepEqual(await page.locator('[data-field=role]').evaluateAll((nodes: HTMLSelectElement[]) => nodes.map(node => node.value)), ['server', 'server', 'server']);
  await page.screenshot({ path: join(dir, 'multi-host.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.screenshot({ path: join(dir, 'mobile-multi-host.png'), fullPage: true });
  await page.setViewportSize({ width: 1280, height: 920 });
  await page.getByRole('radio', { name: /单机 K3d/ }).check();
  assert.equal(await page.locator('#node-section').isVisible(), false);
  await offline.check();
  assert.equal(await page.getByLabel('离线安装包目录', { exact: true }).isVisible(), true);
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await page.getByRole('heading', { name: '设置部署环境', exact: true }).isVisible(), true);
  await online.check();
  assert.equal(await page.getByText('开发选项', { exact: true }).count(), 0);
  assert.equal(await page.getByLabel('本地宸途源码路径', { exact: true }).count(), 0);
  await page.screenshot({ path: join(dir, 'step-1.png'), fullPage: true });
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('heading', { name: '设置组织', exact: true }).waitFor();
  assert.equal(await page.locator('#deployment-options').isVisible(), false);
  assert.equal(await page.getByRole('button', { name: '开始部署', exact: true }).count(), 0);
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await slug.isVisible(), true, 'Empty organization cannot advance');
  await slug.fill('Bad Name');
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await slug.isVisible(), true, 'Invalid organization cannot advance');
  await slug.fill('example-team');
  const domain = page.getByLabel('平台域名', { exact: true });
  assert.equal(await domain.inputValue(), 'example-team.apeironlab.internal');
  assert.equal(await domain.getAttribute('readonly'), '');
  await slug.fill('second-team');
  assert.equal(await domain.inputValue(), 'second-team.apeironlab.internal');
  await page.getByText('自定义域名', { exact: true }).click();
  await page.getByRole('checkbox', { name: '使用自己的域名', exact: true }).check();
  await page.getByLabel('平台域名', { exact: true }).fill('example.internal');
  await slug.fill('example-team');
  assert.equal(await domain.inputValue(), 'example.internal', 'Custom domain survives slug changes');
  await page.getByRole('checkbox', { name: '使用自己的域名', exact: true }).uncheck();
  assert.equal(await domain.inputValue(), 'example-team.apeironlab.internal');
  assert.equal(await page.locator('#setup-form #check-dns').count(), 0, 'Access setup belongs after deployment');
  assert.equal(await page.locator('#check-dns').isVisible(), false);
  assert.equal(await page.locator('#entry-field').isVisible(), false, 'Local K3d does not ask for an entry IP');
  assert.equal(await page.locator('[data-panel]').nth(1).locator('input:visible').count(), 3, 'Organization only has slug, domain and custom-domain switch');
  assert.equal(dnsRequests.length, 0, 'Organization never initiates DNS checks');
  await page.getByRole('checkbox', { name: '使用自己的域名', exact: true }).check();
  await domain.fill('example.internal');
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.screenshot({ path: join(dir, 'mobile-domain.png'), fullPage: true });
  await page.setViewportSize({ width: 1280, height: 920 });
  await page.screenshot({ path: join(dir, 'step-2-organization.png'), fullPage: true });
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('heading', { name: '选择要启用的应用', exact: true }).waitFor();
  const required = ['Nexus', 'Vasi', 'Limani', 'Apeiron'];
  const defaults = ['Task', 'Corpus', 'Chat', 'Files', '邮件'];
  const optional = ['Gateway', 'Filer', '代码仓库', 'GPUStack', 'Langfuse', 'Grafana'];
  const app = (name: string) => page.getByRole('checkbox', { name: new RegExp('^' + name + ' ') });
  assert.deepEqual(await page.locator('.app-copy strong').allTextContents(), [...required, ...defaults, ...optional]);
  for (const name of required) { assert.equal(await app(name).isChecked(), true); assert.equal(await app(name).isDisabled(), true); }
  for (const name of [...defaults, ...optional]) {
    assert.equal(await app(name).isChecked(), defaults.includes(name));
    assert.equal(await app(name).isEnabled(), true);
  }
  assert.equal(await page.locator('#selected-count').textContent(), '已选择 9 个应用（4 个必选）');
  assert.deepEqual(await page.locator('.app-group').allTextContents(), ['必选应用', '可选应用 · 默认选中', '其他可选应用 · 默认不选']);
  await page.screenshot({ path: join(dir, 'step-3-apps.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: join(dir, 'mobile-apps.png'), fullPage: true });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.setViewportSize({ width: 1280, height: 920 });
  for (const name of defaults) await app(name).uncheck();
  for (const name of optional) await app(name).check();
  await page.getByRole('button', { name: '上一步', exact: true }).click();
  assert.equal(await slug.inputValue(), 'example-team');
  await page.getByRole('button', { name: '上一步', exact: true }).click();
  assert.equal(await page.getByRole('radio', { name: /单机 K3d/ }).isChecked(), true);
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await app('Grafana').isChecked(), true);
  assert.equal(await app('Files').isChecked(), false);
  const dockerFixture = await readFile(join(setup.bin, 'docker'), 'utf8');
  await writeFile(join(setup.bin, 'docker'), dockerFixture.replace('"HostPort":"54321"', '"HostPort":"54323"'));
  await page.getByRole('button', { name: '开始部署', exact: true }).click();
  await page.getByText(/已找到本次安装的集群，但入口端口与当前配置不同/).waitFor();
  assert.match(await page.locator('[aria-current=step]').innerText(), /3[\s\S]*应用/);
  assert.equal((await fetch(url + 'api/deployment').then(r => r.json())).phase, 'idle');
  assert.equal(await Bun.file(setup.calls).exists(), false, 'Port rejection never starts prepare or sync');
  await writeFile(join(setup.bin, 'docker'), dockerFixture);
  await page.getByRole('button', { name: '开始部署', exact: true }).click();
  await page.getByRole('heading', { name: '正在部署…', exact: true }).waitFor();
  assert.match(await page.locator('[aria-current=step]').innerText(), /4[\s\S]*部署/);
  assert.equal(await page.locator('[data-step]').nth(4).getAttribute('data-state'), 'pending');
  assert.equal(await page.getByRole('button', { name: '完成并关闭向导' }).isDisabled(), true);
  await page.reload();
  await page.getByRole('heading', { name: '部署失败。', exact: true }).waitFor();
  assert.equal(await page.locator('#access-complete').isVisible(), false);
  assert.equal(await page.locator('#check-dns').isVisible(), false);
  assert.equal(dnsRequests.length, 0, 'Failed installation does not start access configuration');
  assert.equal((await fetch(url + 'api/ca.crt')).status, 404);
  assert.equal(await page.getByText('本地测试 · K3d', { exact: true }).isVisible(), true, 'Reload retains the test-mode indicator');
  assert.equal((await readFile(setup.calls, 'utf8')).trim().split('\n').length, 2, 'Reload does not start another deployment');
  assert.equal((await page.locator('body').textContent())!.includes('private-log-test-key'), false);
  const [logPage] = await Promise.all([
    page.waitForEvent('popup'),
    page.getByRole('link', { name: '在网页打开日志', exact: true }).click(),
  ]);
  await logPage.waitForLoadState();
  assert.equal(logPage.url(), url + 'api/log');
  assert.match(await logPage.locator('body').innerText(), /private-log-test-key/);
  assert.match(await logPage.locator('body').innerText(), /退出码 17/);
  await logPage.close();
  const [logDownload] = await Promise.all([
    page.waitForEvent('download'),
    page.getByRole('link', { name: '下载日志', exact: true }).click(),
  ]);
  assert.equal(logDownload.suggestedFilename(), 'install.log');
  assert.match(await readFile((await logDownload.path())!, 'utf8'), /退出码 17/);
  await page.screenshot({ path: join(dir, 'deployment-failed.png'), fullPage: true });
  const saved = JSON.parse(await readFile(path, 'utf8'));
  assert.equal(saved.schemaVersion, 2);
  assert.equal(saved.deployment.root, '', 'The source override never becomes user installation config');
  assert.equal(saved.slug, 'example-team');
  assert.equal(saved.deployment.installation.entryIp, '127.0.0.1');
  assert.equal(saved.llm, undefined);
  assert.deepEqual(saved.apps, ['nexus', 'vasi', 'ontology', 'apeiron', 'gateway', 'filer', 'git', 'gpustack', 'langfuse', 'kps']);
  assert.equal((await stat(path)).mode & 0o777, 0o600);
  const installCatalogPath = join(setup.root, 'setup/install.json');
  const installCatalog = JSON.parse(await readFile(installCatalogPath, 'utf8'));
  for (const target of Object.values(installCatalog.targets) as any[]) target.environment.fixtureDelay = 30_000;
  await writeFile(installCatalogPath, JSON.stringify(installCatalog));
  for (let attempt = 0; attempt < 2; attempt++) {
    await page.getByRole('button', { name: '重新部署', exact: true }).click();
    await page.getByRole('heading', { name: '正在部署…', exact: true }).waitFor();
    await page.waitForFunction(() => document.querySelector('#deployment-events')?.textContent?.includes('正在同步应用：apeiron'));
    await page.screenshot({ path: join(dir, 'deployment-with-stop.png'), fullPage: true });
    await page.getByRole('button', { name: '停止部署', exact: true }).click();
    await page.getByRole('heading', { name: '部署已停止。', exact: true }).waitFor();
    assert.equal(await page.getByRole('button', { name: '重新部署', exact: true }).isEnabled(), true);
    assert.equal(await page.locator('#access-complete').isVisible(), false);
    assert.match(await page.locator('#deployment-note').innerText(), /不会断点续跑/);
    await page.reload();
    await page.getByRole('heading', { name: '部署已停止。', exact: true }).waitFor();
    assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), saved, 'Stop and retry retain the saved configuration');
  }
  await page.screenshot({ path: join(dir, 'deployment-stopped.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.screenshot({ path: join(dir, 'mobile-stopped.png'), fullPage: true });
  await page.setViewportSize({ width: 1280, height: 920 });
  await page.getByRole('button', { name: '返回修改' }).click();
  assert.equal(await page.getByRole('radio', { name: /单机 K3d/ }).isChecked(), true);
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  for (const name of [...required, ...optional]) assert.equal(await app(name).isChecked(), true);
  for (const name of defaults) assert.equal(await app(name).isChecked(), false);
  await freshInstallFixture(dir, setup, 0);
  await page.getByRole('button', { name: '开始部署', exact: true }).click();
  await page.getByRole('heading', { name: '配置访问', exact: true }).waitFor();
  assert.match(await page.locator('[aria-current=step]').innerText(), /5[\s\S]*配置访问/);
  assert.equal(await page.locator('[data-step]').nth(3).getAttribute('data-state'), 'done');
  assert.equal(await page.locator('#deployment').isVisible(), false);
  assert.equal(await page.getByRole('link', { name: '下载 CA 证书', exact: true }).isVisible(), false, 'Manual setup stays collapsed');
  assert.equal(accessRequests, 0, 'Deployment success never installs system configuration automatically');
  await page.getByRole('button', { name: '一键配置本机访问', exact: true }).click();
  await page.getByText('等待 macOS 系统授权。', { exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: '下一步：测试', exact: true }).isDisabled(), true);
  await page.reload();
  await page.getByRole('heading', { name: '配置访问', exact: true }).waitFor();
  await page.getByText('等待 macOS 系统授权。', { exact: true }).waitFor();
  assert.equal(accessRequests, 1, 'Reload polls the existing installation, never authorizes again');
  accessStatus = { phase: 'cancelled', message: '已取消系统授权，可以重试。' };
  await page.getByRole('button', { name: '重试配置本机访问', exact: true }).click();
  await page.getByText('等待 macOS 系统授权。', { exact: true }).waitFor();
  accessStatus = { phase: 'succeeded', message: '本机解析与 HTTPS 检查通过，可以打开 Apeiron。',
    backup: '/private/etc/apeiron-access.fixture/hosts.before', checks: ['apeiron', 'iam'].map(name => ({ host: name + '.example.internal', passed: true })) };
  await page.getByText('本机解析与 HTTPS 检查通过，可以打开 Apeiron。', { exact: true }).waitFor();
  assert.equal(accessRequests, 2);
  assert.equal(await page.locator('#local-access-checks li').count(), 2);
  await page.screenshot({ path: join(dir, 'access-configured.png'), fullPage: true });
  await page.getByRole('button', { name: '查看部署记录', exact: true }).click();
  await page.getByRole('heading', { name: '部署完成。', exact: true }).waitFor();
  assert.match(await page.locator('[aria-current=step]').innerText(), /4[\s\S]*部署/);
  await page.getByRole('button', { name: '继续配置访问', exact: true }).click();
  await page.getByRole('heading', { name: '配置访问', exact: true }).waitFor();
  await page.getByText('手动配置 / 其他电脑', { exact: true }).click();
  assert.equal(await page.getByRole('link', { name: '下载 CA 证书', exact: true }).isVisible(), true);
  assert.equal(await page.getByRole('link', { name: '下载 hosts 配置', exact: true }).isVisible(), true);
  const caDownload = await fetch(url + 'api/ca.crt');
  assert.equal(caDownload.status, 200);
  assert.match(caDownload.headers.get('content-disposition')!, /attachment/);
  const caText = await caDownload.text();
  assert.match(caText, /BEGIN CERTIFICATE/); assert.doesNotMatch(caText, /PRIVATE KEY/);
  assert.match(await fetch(url + 'api/hosts.txt').then(response => response.text()), /127.0.0.1 task.example.internal/);
  assert.equal(dnsRequests.length, 0, 'Successful deployment waits for the user to configure access');
  await page.getByRole('button', { name: '检测解析', exact: true }).click();
  await page.getByText('解析未通过，请按配置说明检查 DNS / hosts。', { exact: true }).waitFor();
  assert.equal(await page.locator('#dns-guide').getAttribute('open'), '');
  assert.match(await page.locator('#dns-records').innerText(), /127.0.0.1 apeiron.example.internal/);
  assert.equal(await page.getByRole('heading', { name: '配置访问', exact: true }).isVisible(), true, 'DNS failure does not turn installation into a failure');
  dnsPassed = true;
  await page.getByRole('button', { name: '检测解析', exact: true }).click();
  await page.getByText('CLI 主机 fixture-mac 解析通过', { exact: true }).waitFor();
  assert.deepEqual(dnsRequests, Array(2).fill({ domain: 'example.internal', entryIp: '127.0.0.1', local: true }));
  await page.getByText('信任 CA 证书', { exact: true }).click();
  assert.match(await page.locator('#ca-fingerprint').innerText(), /^(?:[A-F0-9]{2}:){31}[A-F0-9]{2}$/);
  desktopAvailable = false;
  await page.reload();
  await page.getByRole('heading', { name: '配置访问', exact: true }).waitFor();
  await page.getByText('远程主机没有 Mac 桌面，请使用手动配置。', { exact: true }).waitFor();
  assert.equal(await page.locator('#install-access').isVisible(), false);
  assert.equal(await page.getByRole('link', { name: '下载 CA 证书', exact: true }).isVisible(), false);
  desktopAvailable = true;
  await page.reload();
  await page.getByRole('heading', { name: '配置访问', exact: true }).waitFor();
  await page.getByText('本机解析与 HTTPS 检查通过，可以打开 Apeiron。', { exact: true }).waitFor();
  assert.equal(accessRequests, 2);
  assert.equal(await page.getByText('本地测试 · K3d', { exact: true }).isVisible(), true);
  const retried = JSON.parse(await readFile(path, 'utf8'));
  assert.equal(retried.deployment.installation.topology, 'single-k3d');
  assert.equal(retried.deployment.runner, 'docker');
  assert.equal('profile' in retried.deployment, false);
  assert.deepEqual((await readFile(setup.calls, 'utf8')).trim().split('\n').map(line => JSON.parse(line).args), [...Array.from({ length: 4 }, () => [['prepare'], ['sync']]).flat(), ['configure-oidc']]);
  assert.equal((await readFile(setup.calls, 'utf8')).trim().split('\n').length, 9);
  assert.equal((await readFile(setup.calls, 'utf8')).trim().split('\n').every(line => JSON.parse(line).httpsPort === '54321'), true);
  await page.screenshot({ path: join(dir, 'deployment-complete.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: join(dir, 'mobile-deployment.png'), fullPage: true });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.setViewportSize({ width: 1280, height: 980 });
  await page.getByRole('button', { name: '下一步：测试', exact: true }).click();
  await page.getByRole('heading', { name: '测试', exact: true }).waitFor();
  await page.getByText('凭据已就绪。密码默认隐藏，可显示或复制。', { exact: true }).waitFor();
  assert.match(await page.locator('[aria-current=step]').innerText(), /6[\s\S]*测试/);
  assert.equal(await page.getByLabel('用户名', { exact: true }).inputValue(), 'fixture-admin');
  assert.equal(await page.getByLabel('密码', { exact: true }).inputValue(), 'fixture-only-admin-password');
  assert.equal(await page.getByLabel('密码', { exact: true }).getAttribute('type'), 'password');
  assert.equal((await page.content()).includes('fixture-only-admin-password'), false, 'Password is not serialized into HTML');
  await page.getByRole('button', { name: '显示密码', exact: true }).click();
  assert.equal(await page.getByLabel('密码', { exact: true }).getAttribute('type'), 'text');
  await page.getByRole('button', { name: '隐藏密码', exact: true }).click();
  await page.getByRole('button', { name: '复制用户名', exact: true }).click();
  assert.equal(await page.evaluate(() => (window as any).__copied), 'fixture-admin');
  await page.getByRole('button', { name: '复制密码', exact: true }).click();
  assert.equal(await page.evaluate(() => (window as any).__copied), 'fixture-only-admin-password');
  const [adminDownload] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: '下载凭据', exact: true }).click()]);
  assert.equal(adminDownload.suggestedFilename(), 'apeiron-admin.json');
  assert.deepEqual(JSON.parse(await readFile((await adminDownload.path())!, 'utf8')), { username: 'fixture-admin', password: 'fixture-only-admin-password' });
  assert.equal(await page.getByRole('link', { name: '登录 Apeiron ↗', exact: true }).getAttribute('href'), 'https://apeiron.example.internal:54321/');
  assert.equal(await page.getByRole('link', { name: '登录 Ops ↗', exact: true }).getAttribute('href'), 'https://ops.example.internal:54321/');
  await page.getByRole('button', { name: '开始测试', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#verification-status')?.getAttribute('data-state') === 'failed');
  assert.match(await page.locator('#verification-results').innerText(), /HTTP 404/);
  verificationPassed = true;
  await page.getByRole('button', { name: '开始测试', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#verification-status')?.getAttribute('data-state') === 'passed');
  await page.screenshot({ path: join(dir, 'step-6-tests.png'), fullPage: true });
  await page.getByRole('button', { name: '返回配置访问', exact: true }).click();
  assert.equal(await page.locator('#admin-password').inputValue(), '', 'Leaving tests clears the secret');
  await page.getByRole('button', { name: '下一步：测试', exact: true }).click();
  await page.getByText('凭据已就绪。密码默认隐藏，可显示或复制。', { exact: true }).waitFor();
  await page.reload();
  await page.getByRole('heading', { name: '测试', exact: true }).waitFor();
  await page.getByText('凭据已就绪。密码默认隐藏，可显示或复制。', { exact: true }).waitFor();
  assert.equal(await page.getByLabel('密码', { exact: true }).getAttribute('type'), 'password');
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: join(dir, 'mobile-tests.png'), fullPage: true });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.getByRole('button', { name: '完成并关闭向导', exact: true }).click();
  await page.getByRole('button', { name: '向导已关闭', exact: true }).waitFor();
  assert.equal(await page.locator('#admin-password').inputValue(), '');
  assert.equal(await child.exited, 0);
  assert.deepEqual(errors, []); assert.deepEqual(externalRequests, []);
  let output = startup;
  const remaining = child.stdout.getReader();
  for (;;) { const chunk = await remaining.read(); if (chunk.done) break; output += new TextDecoder().decode(chunk.value); }
  remaining.releaseLock(); output += await new Response(child.stderr).text();
  assert.equal(output.includes('private-log-test-key'), false);
  console.log(`PASS compiled CLI + fixture deployer: six-step setup, initial admin display/copy/download, connection tests, separate deployment/access, local installation/cancellation/retry, machine probe/offline switch, selections, failure/retry, browser log view/download, reload, finish, desktop/mobile\nscreenshots: ${dir}`);
} finally {
  clearTimeout(timeout); await browser?.close(); child.kill(); await child.exited;
  // Preserve screenshots only; fixture files and logs contain no real credentials.
  for (const entry of ['config.json', 'deployments', 'chentu fixture', 'work', 'bin', 'base.yaml', 'calls.jsonl', 'fixture.ts', 'fixture-ca.crt', 'fixture-ca.key']) await rm(join(dir, entry), { recursive: true, force: true });
}
