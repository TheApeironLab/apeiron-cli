// secrets: none
// C-DEP-65 clinview wizard @gate @cap-DEP; deterministic fixtures, no model calls.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { startInitServer } from '../test/rust-server';
import { deploymentFixture, freshInstallFixture } from '../test/fixtures';

const dir = await mkdtemp(join(tmpdir(), 'apeiron-clinview-ui-'));
const setup = await deploymentFixture(dir);
await freshInstallFixture(dir, setup);
await mkdir(join(dir, 'home'));
const catalogPath = join(setup.root, 'setup/install.json');
const catalog = JSON.parse(await readFile(catalogPath, 'utf8'));
catalog.distro = 'clinview';
catalog.applications = catalog.applications.filter(app => ['nexus', 'vasi', 'apeiron'].includes(app.id));
catalog.components.clinview = { requires: ['core'], files: [] };
catalog.applications.push({ id: 'clinview', name: 'Clinview', description: '病例审阅', required: true, releases: ['clinview'], requires: [] });
for (const target of Object.values(catalog.targets)) target.environment.distro = 'clinview';
await writeFile(catalogPath, JSON.stringify(catalog));
const server = await startInitServer({ path: join(dir, 'config.json'), env: {
    HOME: join(dir, 'home'), APEIRON_CHENTU_ROOT: setup.root
} });
const browser = await chromium.launch({ headless: true });
try {
    const page = await browser.newPage();
    await page.route('**/api/probe', route => route.fulfill({ json: {
        machine: { os: { name: 'macOS', version: '26' }, hardware: {
            architecture: 'arm64', runtimeArchitecture: 'arm64', cores: 8, memoryGiB: 32, cpu: 'Fixture'
        } }, network: { status: 'skipped', checks: [] }, checkedAt: '2026-10-25T00:00:00Z'
    } }));
    let preflightInput;
    await page.route('**/api/preflight', route => {
        preflightInput = route.request().postDataJSON();
        return route.fulfill({ json: { preflight: 'fixture ready', blocked: null } });
    });
    await page.route('**/api/models/list', route => route.fulfill({ json: { models: ['fixture-chat'] } }));
    await page.route('**/api/models/test', route => route.fulfill({ json: { results: [
        { mode: 'fast', elapsedMs: 10 }, { mode: 'deep', elapsedMs: 20 }
    ] } }));
    await page.goto(server.url);
    await page.getByRole('button', { name: '直接配置当前实例（不新建）' }).click();
    await page.getByRole('radio', { name: /单机 K3d/ }).check();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    for (const name of ['Nexus', 'Vasi', 'Apeiron', 'Clinview']) {
        const checkbox = page.getByRole('checkbox', { name: new RegExp('^' + name + ' ') });
        assert.equal(await checkbox.isChecked(), true);
        assert.equal(await checkbox.isDisabled(), true);
    }
    assert.equal(await page.getByRole('checkbox', { name: /^Gateway / }).count(), 0);
    assert.match(await page.getByRole('main').innerText(), /Clinview：[\s\S]*本机使用包含 scode/);
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    assert.match(await page.getByRole('main').innerText(), /Apeiron 直接连接模型服务/);
    await page.getByRole('textbox', { name: 'Slug name', exact: true }).fill('clinview-test');
    await page.getByRole('textbox', { name: 'Base URL', exact: true }).fill('https://models.example.internal/v1');
    await page.getByLabel(/^API Key/).fill('fixture-only-clinview-key');
    await page.getByRole('button', { name: '获取模型列表', exact: true }).click();
    await page.getByRole('combobox', { name: '快速 · apeiron-flash' }).selectOption('fixture-chat');
    await page.getByRole('combobox', { name: '深度思考 · apeiron-pro' }).selectOption('fixture-chat');
    await page.getByRole('button', { name: '测试两种模式', exact: true }).click();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('heading', { name: '预检与部署摘要', exact: true }).waitFor();
    assert.equal((await page.getByRole('main').innerText()).includes('fixture-only-clinview-key'), false);
    assert.equal(preflightInput.deployment.installation.distro, 'clinview');
    assert.deepEqual(preflightInput.apps.sort(), ['apeiron', 'clinview', 'nexus', 'vasi']);
    console.log('PASS C-DEP-65 clinview wizard @gate @cap-DEP: required apps, bundled scode, direct models, redacted summary');
} finally {
    await browser.close();
    await server.stop();
    await rm(dir, { recursive: true, force: true });
}
