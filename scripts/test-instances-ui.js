import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
// Browser verification of the instance landing page and list: create, switch,
// diagnostics download and uninstall against a compiled CLI with docker/k3d stubs.
const dir = await mkdtemp(join(tmpdir(), 'apeiron-instances-ui-'));
const home = join(dir, 'home');
const bin = join(dir, 'bin');
await mkdir(bin, { recursive: true });
await mkdir(home, { recursive: true });
// docker: every k3d load balancer is "stopped"; k3d: record argv and succeed.
await writeFile(join(bin, 'docker'), '#!/bin/sh\nif [ "$1" = inspect ]; then echo false; exit 0; fi\nexit 0\n', { mode: 0o755 });
await writeFile(join(bin, 'k3d'), `#!/bin/sh\necho "$@" >> ${JSON.stringify(join(dir, 'k3d.log'))}\nexit 0\n`, { mode: 0o755 });
// A packaged-style path (instances/<name>/config.json) is adopted as the "default" instance once saved.
const path = join(home, '.apeiron/installers/cand/instances/default/config.json');
const binary = process.argv[2] ? resolve(process.argv[2]) : resolve(import.meta.dir, '../target/release/apeiron');
const child = Bun.spawn([binary, 'init', '--no-open', '--config', path], {
    cwd: dir, stdout: 'pipe', stderr: 'pipe', env: { ...process.env, HOME: home, PATH: bin + ':' + process.env.PATH },
});
let browser;
const timeout = setTimeout(() => { child.kill(); throw new Error('Browser verification timed out'); }, 90_000);
try {
    let startup = '';
    const reader = child.stdout.getReader();
    let url = '';
    while (!url) {
        const result = await reader.read();
        if (result.done)
            throw new Error('Wizard exited before printing its URL: ' + await new Response(child.stderr).text());
        startup += new TextDecoder().decode(result.value);
        url = /^url\t(.+)$/m.exec(startup)?.[1] ?? '';
    }
    reader.releaseLock();
    assert.match(startup, /^config\t/m, 'start contract keeps the config line');
    assert.match(startup, /^forward\tssh -L /m);
    const origin = new URL(url).origin;
    const post = (route, body) => fetch(url + 'api/' + route, { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: origin }, body: JSON.stringify(body) });
    // Save a K3d configuration so the packaged path registers as "default" with its ports.
    const defaults = (await fetch(url + 'api/config').then(r => r.json())).defaults;
    const saved = await post('config', { slug: 'default', apps: ['nexus', 'vasi', 'ontology', 'apeiron', 'gateway'], revision: null,
        deployment: { ...defaults, offline: false, bundleDir: '', installation: { topology: 'single-k3d', domain: 'default.apeironlab.internal', entryIp: '127.0.0.1', httpPort: 54320, httpsPort: 54321, ha: false, sshUser: '', sshKey: '', sshPort: 22, nodes: [] } } });
    assert.equal(saved.status, 200, await saved.text());
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage({ viewport: { width: 1280, height: 920 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/api/probe', route => route.fulfill({ contentType: 'application/json', body: JSON.stringify({
        machine: { os: { name: 'Ubuntu', version: '22.04', kernel: '6.8.0-fixture' }, hardware: { architecture: 'amd64', runtimeArchitecture: 'amd64', cpu: 'Fixture CPU', cores: 8, memoryGiB: 16 } },
        checkedAt: new Date().toISOString(), network: { status: 'skipped', checks: [] } }) }));
    await page.route('**/api/applications', route => route.fulfill({ json: { apps: ['nexus', 'vasi', 'apeiron'].map(id => ({ id, name: id, description: id, required: true, releases: [id], requires: [] })), distro: 'minimal' } }));
    await page.goto(url);
    await page.getByRole('heading', { name: '选择要进行的操作', exact: true }).waitFor();
    assert.match(await page.locator('#instance-chip').innerText(), /^default$/, 'The adopted packaged instance is shown as current');
    assert.match(await page.locator('#home-instance-count').innerText(), /1 个实例/);
    await page.screenshot({ path: join(dir, 'home.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.screenshot({ path: join(dir, 'mobile-home.png'), fullPage: true });
    await page.setViewportSize({ width: 1280, height: 920 });
    // Instance list: the current instance cannot be uninstalled; the state shows phase and k3d.
    await page.getByRole('button', { name: '查看实例列表', exact: true }).click();
    await page.locator('.instance-card').waitFor();
    assert.equal(await page.locator('.instance-card').count(), 1);
    assert.equal(await page.locator('.instance-card[data-current=true] strong').first().innerText(), 'default');
    assert.match(await page.locator('.instance-card .instance-state').innerText(), /已配置 · 集群已停止/);
    assert.match(await page.locator('.instance-card .instance-meta').innerText(), /default\.apeironlab\.internal[\s\S]*54320 \/ 54321/);
    assert.equal(await page.locator('.instance-card').getByRole('button', { name: '卸载', exact: true }).isDisabled(), true);
    assert.equal(await page.locator('.instance-card').getByRole('button', { name: '继续配置', exact: true }).isEnabled(), true);
    await page.locator('.instance-card').getByRole('button', { name: '诊断采集', exact: true }).click();
    await page.getByText('该实例尚未生成安装日志。', { exact: true }).waitFor();
    await page.screenshot({ path: join(dir, 'instances.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.screenshot({ path: join(dir, 'mobile-instances.png'), fullPage: true });
    await page.setViewportSize({ width: 1280, height: 920 });
    // Create a second K3d instance from the landing page: ports are allocated server-side.
    await page.getByRole('button', { name: '返回首页', exact: true }).click();
    await page.getByLabel('实例名称', { exact: true }).fill('Bad Name');
    await page.getByRole('button', { name: '创建并开始配置', exact: true }).click();
    await page.getByText('请填写有效的实例名称。', { exact: true }).waitFor();
    await page.getByLabel('实例名称', { exact: true }).fill('lab');
    await page.getByRole('button', { name: '创建并开始配置', exact: true }).click();
    await page.getByRole('heading', { name: '选择部署位置', exact: true }).waitFor();
    assert.match(await page.locator('#instance-chip').innerText(), /^lab$/);
    assert.match(await page.locator('#instance-chip').getAttribute('title'), /实例 ID：[a-f0-9]{12}/);
    assert.equal(await page.getByRole('radio', { name: /Docker 中运行 K3d/ }).isChecked(), true);
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('heading', { name: '选择发行版', exact: true }).waitFor();
    const labHttpPort = Number(await page.locator('#http-port').inputValue());
    assert.ok(labHttpPort >= 54322, 'Allocate available ports after the adopted default instance');
    assert.equal(await page.locator('#https-port').inputValue(), String(labHttpPort + 1));
    assert.equal(await page.locator('#http-port').getAttribute('readonly'), '');

    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('heading', { name: '共存配置与资源预算', exact: true }).waitFor();
    assert.match(await page.locator('#coexist-instance').innerText(), /^lab$/);
    assert.match(await page.locator('#coexist-config').innerText(), /instances\/lab\/config\.json$/);
    assert.match(await page.locator('#coexist-ports').innerText(), new RegExp(`HTTP ${labHttpPort} / HTTPS ${labHttpPort + 1}`));
    const registry = JSON.parse(await readFile(join(home, '.apeiron/instances.json'), 'utf8'));
    assert.deepEqual(registry.instances.map(i => [i.name, i.httpPort]), [['default', 54320], ['lab', labHttpPort]]);
    // Back to the list: default is no longer current, so its stopped cluster can be uninstalled.
    await page.getByRole('button', { name: '返回首页', exact: true }).click();
    await page.getByRole('button', { name: '查看实例列表', exact: true }).click();
    await page.locator('.instance-card[data-current=true]').waitFor();
    assert.equal(await page.locator('.instance-card').count(), 2);
    assert.equal(await page.locator('.instance-card[data-current=true] strong').first().innerText(), 'lab');
    const defaultCard = page.locator('.instance-card').filter({ hasText: 'default' });
    await defaultCard.getByRole('button', { name: '卸载', exact: true }).click();
    const confirmation = page.getByRole('dialog', { name: '卸载实例？', exact: true });
    await confirmation.waitFor();
    assert.equal(await confirmation.evaluate(dialog => dialog.matches(':modal')), true, 'Confirmation blocks the background');
    assert.match(await confirmation.innerText(), /将卸载实例 default/);
    assert.equal(await page.locator('#instance-cancel-delete').evaluate(button => button === document.activeElement), true);
    const assertCentered = async () => {
        const box = await confirmation.boundingBox();
        const viewport = page.viewportSize();
        assert.ok(box.x >= 0 && box.y >= 0 && box.x + box.width <= viewport.width && box.y + box.height <= viewport.height, 'Confirmation fits the viewport');
        assert.ok(Math.abs(box.y + box.height / 2 - viewport.height / 2) < 2, 'Confirmation stays centered regardless of list height');
    };
    await assertCentered();
    await page.screenshot({ path: join(dir, 'uninstall-dialog.png') });
    await page.keyboard.press('Escape');
    await confirmation.waitFor({ state: 'hidden' });
    assert.equal(await defaultCard.getByRole('button', { name: '卸载', exact: true }).evaluate(button => button === document.activeElement), true, 'Closing restores focus');
    await page.setViewportSize({ width: 390, height: 844 });
    await defaultCard.getByRole('button', { name: '卸载', exact: true }).click();
    await assertCentered();
    await page.screenshot({ path: join(dir, 'mobile-uninstall-dialog.png') });
    await confirmation.getByRole('button', { name: '取消', exact: true }).click();
    await confirmation.waitFor({ state: 'hidden' });
    await page.setViewportSize({ width: 1280, height: 920 });
    assert.equal(await page.locator('.instance-card').count(), 2, 'Cancel keeps the instance');
    assert.equal(await Bun.file(join(dir, 'k3d.log')).exists(), false);
    await defaultCard.getByRole('button', { name: '卸载', exact: true }).click();
    // Failure stays visible in the modal, and retry still targets the same instance.
    await page.route('**/api/instances/delete', route => route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: '实例暂时无法卸载，请重试。' }) }), { times: 1 });
    await confirmation.getByRole('button', { name: '确认卸载', exact: true }).click();
    await confirmation.getByText('实例暂时无法卸载，请重试。', { exact: true }).waitFor();
    assert.equal(await confirmation.isVisible(), true);
    assert.equal(await Bun.file(join(dir, 'k3d.log')).exists(), false);
    await confirmation.getByRole('button', { name: '确认卸载', exact: true }).click();
    await confirmation.waitFor({ state: 'hidden' });
    await page.getByText(/已卸载 default，并删除其 K3d 集群/).waitFor();
    assert.equal(await page.locator('.instance-card').count(), 1);
    const k3dCalls = (await readFile(join(dir, 'k3d.log'), 'utf8')).trim().split('\n');
    assert.match(k3dCalls[0], /^cluster delete apeiron-[a-f0-9]{12}$/);
    assert.match(k3dCalls[1], /^registry delete k3d-apeiron-[a-f0-9]{12}-reg$/);
    assert.equal(await Bun.file(path).exists(), false, 'Uninstall removes the instance directory');
    // Switching back into the only remaining instance continues its configuration.
    await page.locator('.instance-card').getByRole('button', { name: '继续配置', exact: true }).click();
    await page.getByRole('heading', { name: '选择部署位置', exact: true }).waitFor();
    assert.deepEqual(errors, []);
    console.log(`PASS instance landing and list: adopted packaged instance, create with server-side ports, switch, diagnostics, uninstall confirmation, desktop/mobile\nscreenshots: ${dir}`);
}
finally {
    clearTimeout(timeout);
    await browser?.close();
    child.kill();
    await child.exited;
    for (const entry of ['home', 'bin', 'k3d.log'])
        await rm(join(dir, entry), { recursive: true, force: true });
}
