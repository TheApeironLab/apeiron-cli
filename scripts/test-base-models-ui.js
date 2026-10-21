// secrets: none
// Deterministic browser acceptance; model endpoints are intercepted, never real models.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { startInitServer } from '../test/rust-server';
const dir = await mkdtemp(join(tmpdir(), 'apeiron-base-models-'));
await mkdir(join(dir, 'home'));
const server = await startInitServer({ path: join(dir, 'config.json'), env: { HOME: join(dir, 'home') } });
const browser = await chromium.launch({ headless: true });
try {
    const page = await browser.newPage();
    await page.route('**/api/probe', route => route.fulfill({ json: {
        machine: { os: { name: 'macOS', version: '26' }, hardware: { architecture: 'arm64', runtimeArchitecture: 'arm64', cores: 8, memoryGiB: 32, cpu: 'Fixture' } },
        network: { status: 'skipped', checks: [] }, checkedAt: '2026-10-21T00:00:00Z'
    } }));
    await page.route('**/api/preflight', route => route.fulfill({ json: { preflight: 'fixture ready', blocked: null } }));
    await page.route('**/api/models/list', route => route.fulfill({ json: { models: ['qwen-plus'] } }));
    await page.route('**/api/models/test', route => route.fulfill({ json: { results: [
        { mode: 'fast', elapsedMs: 1 }, { mode: 'deep', elapsedMs: 1 }
    ] } }));
    await page.goto(server.url);
    await page.getByRole('button', { name: '直接配置当前实例（不新建）' }).click();
    await page.getByRole('radio', { name: /单机 K3d/ }).check();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    for (const name of ['Nexus', 'Vasi', 'Apeiron', 'Gateway']) {
        const checkbox = page.getByRole('checkbox', { name: new RegExp('^' + name + ' ') });
        assert.equal(await checkbox.isChecked(), true);
        assert.equal(await checkbox.isDisabled(), true);
    }
    for (const name of ['Limani', 'Task', 'Corpus', 'Chat', 'Files', '邮件'])
        assert.equal(await page.getByRole('checkbox', { name: new RegExp('^' + name + ' ') }).isChecked(), false);
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    const provider = page.getByRole('textbox', { name: 'Provider', exact: true });
    assert.equal(await provider.inputValue(), 'bailian');
    assert.equal(await page.getByRole('textbox', { name: 'Base URL', exact: true }).inputValue(), 'https://dashscope.aliyuncs.com/compatible-mode/v1');
    assert.equal(await page.getByRole('checkbox', { name: /稍后配置/ }).count(), 0);
    const key = page.getByLabel(/^API Key/);
    assert.equal(await key.getAttribute('type'), 'password');
    await page.getByRole('textbox', { name: 'Slug name', exact: true }).fill('base-test');
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    assert.equal(await page.getByRole('heading', { name: '域名、访问方式与模型', exact: true }).isVisible(), true);
    await key.fill('fixture-only-key');
    await page.getByRole('button', { name: '获取模型列表', exact: true }).click();
    await page.getByRole('combobox', { name: '快速 · apeiron-flash' }).selectOption('qwen-plus');
    await page.getByRole('combobox', { name: '深度思考 · apeiron-pro' }).selectOption('qwen-plus');
    await page.getByRole('button', { name: '测试两种模式', exact: true }).click();
    await page.getByText(/快速：通过/).waitFor();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('heading', { name: '预检与部署摘要', exact: true }).waitFor();
    assert.equal((await page.getByRole('main').innerText()).includes('fixture-only-key'), false);
    console.log('PASS base wizard: four required system apps, Bailian defaults, password input, mandatory model setup, deterministic model discovery/test, redacted summary');
} finally {
    await browser.close();
    await server.stop();
    await rm(dir, { recursive: true, force: true });
}
