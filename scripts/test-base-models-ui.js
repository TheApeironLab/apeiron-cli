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
const screenshots = await mkdtemp(join(tmpdir(), 'apeiron-model-results-'));
try {
    const page = await browser.newPage();
    await page.route('**/api/probe', route => route.fulfill({ json: {
        machine: { os: { name: 'macOS', version: '26' }, hardware: { architecture: 'arm64', runtimeArchitecture: 'arm64', cores: 8, memoryGiB: 32, cpu: 'Fixture' } },
        network: { status: 'skipped', checks: [] }, checkedAt: '2026-10-21T00:00:00Z'
    } }));
    await page.route('**/api/preflight', route => route.fulfill({ json: { preflight: 'fixture ready', blocked: null } }));
    await page.route('**/api/models/list', route => route.fulfill({ json: { models: ['qwen-plus'] } }));
    let modelReply = 'pending', releaseTest;
    const modelGate = new Promise(resolve => { releaseTest = resolve; });
    await page.route('**/api/models/test', async route => {
        if (modelReply === 'pending') await modelGate;
        await route.fulfill(modelReply === 'error' ? { status: 502, json: { error: '模型服务暂时不可用，请稍后重试。' } } : { json: { results: [
            { mode: 'fast', elapsedMs: 400 }, { mode: 'deep', elapsedMs: 1250 }
        ] } });
    });
    await page.route('**/api/applications', route => route.fulfill({ json: { apps: ['nexus', 'vasi', 'apeiron'].map(id => ({ id, name: id, description: id, required: true, releases: [id], requires: [] })), distro: 'minimal' } }));
    await page.goto(server.url);
    await page.getByRole('button', { name: '直接配置当前实例（不新建）' }).click();
    await page.getByRole('radio', { name: /Docker 中运行 K3d/ }).check();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    assert.equal(await page.getByRole('radio', { name: /^Minimal/ }).isChecked(), true);
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
    await page.locator('#model-status[data-state=pending]').waitFor();
    assert.equal(await page.getByRole('button', { name: '测试两种模式', exact: true }).isDisabled(), true);
    modelReply = 'success';
    releaseTest();
    await page.locator('#model-status[data-state=success]').waitFor();
    assert.equal(await page.locator('.model-result-card').count(), 2);
    assert.match(await page.locator('.model-result-card').first().innerText(), /快速：通过[\s\S]*qwen-plus[\s\S]*耗时 0.4 秒/);
    assert.match(await page.locator('.model-result-card').last().innerText(), /深度思考：通过[\s\S]*耗时 1.3 秒/);
    await page.locator('#model-status').screenshot({ path: join(screenshots, 'success.png') });
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    const firstCard = await page.locator('.model-result-card').first().boundingBox();
    const lastCard = await page.locator('.model-result-card').last().boundingBox();
    assert.ok(lastCard.y >= firstCard.y + firstCard.height, 'Mobile results stack vertically');
    await page.locator('#model-status').screenshot({ path: join(screenshots, 'mobile-success.png') });
    await page.setViewportSize({ width: 1280, height: 920 });
    await provider.fill('custom');
    assert.equal(await page.locator('.model-result-card').count(), 0, 'Configuration changes clear old results');
    modelReply = 'error';
    await page.getByRole('button', { name: '测试两种模式', exact: true }).click();
    await page.locator('#model-status[data-state=error]').waitFor();
    assert.match(await page.locator('#model-status').innerText(), /模型测试未通过[\s\S]*模型服务暂时不可用/);
    assert.equal(await page.locator('.model-result-card').count(), 0);
    await page.locator('#model-status').screenshot({ path: join(screenshots, 'failure.png') });
    modelReply = 'success';
    await page.getByRole('button', { name: '测试两种模式', exact: true }).click();
    await page.locator('#model-status[data-state=success]').waitFor();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('heading', { name: '预检与部署摘要', exact: true }).waitFor();
    assert.equal((await page.getByRole('main').innerText()).includes('fixture-only-key'), false);
    console.log(`Model result screenshots: ${screenshots}`);
    console.log('PASS minimal wizard: fixed distro, Bailian defaults, password input, mandatory model setup, deterministic model discovery/test, redacted summary');
} finally {
    await browser.close();
    await server.stop();
    await rm(dir, { recursive: true, force: true });
}
