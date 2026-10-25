// secrets: none
// Deployment location: real Rust wizard; SSH probe is a deterministic HTTP fixture.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { startInitServer } from '../test/rust-server';
import { deploymentFixture, freshInstallFixture } from '../test/fixtures';
const dir = await mkdtemp(join(tmpdir(), 'apeiron-location-ui-'));
const setup = await deploymentFixture(dir);
await freshInstallFixture(dir, setup);
await mkdir(join(dir, 'home'));
const server = await startInitServer({ path: join(dir, 'config.json'), env: { HOME: join(dir, 'home'), APEIRON_CHENTU_ROOT: setup.root } });
const browser = await chromium.launch({ headless: true });
try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.route('**/api/probe', route => route.fulfill({ json: {
        machine: { os: { name: 'macOS', version: '26' }, hardware: { architecture: 'arm64', runtimeArchitecture: 'arm64', cores: 8, memoryGiB: 32, cpu: 'Fixture' } },
        network: { status: 'skipped', checks: [] }, checkedAt: '2026-10-25T00:00:00Z'
    } }));
    await page.route('**/api/nodes', route => {
        const { hosts } = route.request().postDataJSON();
        return route.fulfill({ json: { nodes: hosts.map((host, i) => ({ host, name: `node-${i}`, os: 'Ubuntu', version: '22.04', architecture: 'amd64', cores: 4, memoryGiB: 8, diskGiB: 80, addresses: [`192.0.2.${10+i}`] })) } });
    });
    await page.goto(server.url);
    assert.equal(await page.getByRole('combobox', { name: '部署位置', exact: true }).count(), 0);
    assert.equal(await page.getByRole('combobox', { name: '本机运行方式' }).count(), 0);
    await page.getByRole('textbox', { name: '实例名称' }).fill('ssh-preview');
    await page.screenshot({ path: '/tmp/apeiron-location-home.png', fullPage: true });
    await page.getByRole('button', { name: '创建并开始配置' }).click();
    await page.getByRole('heading', { name: '选择部署位置' }).waitFor();
    const draft = await fetch(server.url + 'api/config').then(r => r.json());
    assert.equal(draft.config.deployment, null);
    assert.equal(draft.instance.topology, null);
    assert.equal(draft.instance.httpPort, null);
    await page.getByRole('radio', { name: /远端机器 · SSH/ }).check();
    assert.equal(await page.getByRole('radio', { name: /Docker 中运行 K3d/ }).isVisible(), false);
    await page.getByRole('textbox', { name: 'IP 地址或 SSH 别名' }).fill('node-a');
    await page.getByRole('button', { name: '自动检测节点', exact: true }).click();
    await page.getByText('已生成节点配置', { exact: false }).waitFor();
    assert.equal(await page.getByRole('textbox', { name: '节点名称', exact: true }).count(), 1);
    assert.equal(await page.getByRole('checkbox', { name: '控制平面高可用' }).isVisible(), false);
    await page.getByRole('textbox', { name: 'IP 地址或 SSH 别名' }).fill('node-a\nnode-b\nnode-c');
    await page.getByRole('button', { name: '自动检测节点', exact: true }).click();
    await page.getByText('已生成节点配置', { exact: false }).waitFor();
    assert.equal(await page.getByRole('textbox', { name: '节点名称', exact: true }).count(), 3);
    assert.equal(await page.getByRole('checkbox', { name: '控制平面高可用' }).isVisible(), true);
    assert.equal((await page.locator('header.top').innerText()).includes('本地测试'), false);
    assert.match(await page.locator('header.top').innerText(), /SSH · 3 台节点/);
    assert.doesNotMatch(await page.locator('header.top').innerText(), /[a-f0-9]{12}/);
    await page.screenshot({ path: '/tmp/apeiron-location-ssh.png', fullPage: true });
    await page.route('**/api/applications', route => route.fulfill({ json: { apps: ['nexus', 'vasi', 'apeiron'].map(id => ({ id, name: id, description: id, required: true, releases: [id], requires: [] })), distro: 'minimal' } }));
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('heading', { name: '选择发行版', exact: true }).waitFor();
    const state = await fetch(server.url + 'api/config').then(r => r.json());
    assert.equal(state.config.deployment.installation.location, 'ssh');
    assert.equal(state.config.deployment.installation.topology, 'multi-k3s');
    assert.equal(state.config.deployment.installation.nodes.length, 3);
    assert.equal(state.instance.topology, 'multi-k3s');
    assert.equal(state.instance.httpPort, 80);
    assert.deepEqual(errors, []);
    console.log('PASS: name-only creation, deferred target, persisted SSH instance, one/three nodes, target label, no page errors');
} finally {
    await browser.close();
    await server.stop();
    await rm(dir, { recursive: true, force: true });
}
