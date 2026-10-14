import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { renderPage } from '../src/init/page';

const embeddedPage = await Bun.file(new URL('../runtime/assets/setup.html', import.meta.url)).text();
assert.equal(embeddedPage, renderPage('APEIRON_NONCE_PLACEHOLDER'), 'Committed runtime page must match the frontend before exercising deployment controls');

const ids = ['check', 'cluster', 'nexus', 'import', 'applications', 'access'];
const labels = ['检查安装包', '准备集群', '启动制品仓库', '导入部署资源', '安装与初始化应用', '准备访问与验证'];
const directory = process.argv[2] || await mkdtemp(join(tmpdir(), 'apeiron-stages-ui-'));
function snapshot(index, phase = 'running') {
    return { phase, stage: ids[index], message: '正在处理当前部署资源。', events: ['[INFO] 部署状态已更新。'],
        startedAt: new Date().toISOString(),
        stages: ids.map((id, position) => ({ id, state: position < index ? 'done' : position === index ? 'active' : 'pending', detail: position === index ? '当前执行内容 <script>不会作为 HTML 执行</script>' : '' })) };
}
let status = snapshot(0, 'preparing');
let retries = 0;
let resumes = 0;
const defaults = { offline: true, bundleDir: '/offline/bundle', installation: { topology: 'single-k3d', domain: 'example.internal', entryIp: '127.0.0.1', nodes: [], httpPort: 54320, httpsPort: 54321 } };
const apps = await Bun.file(new URL('../runtime/assets/apps.json', import.meta.url)).json();
const server = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === '/') return new Response(embeddedPage.replaceAll('APEIRON_NONCE_PLACEHOLDER', 'test-nonce'), { headers: { 'Content-Type': 'text/html' } });
    if (path === '/api/config') return Response.json({ apps, defaults, revision: 'fixture-revision', config: null, host: { name: 'fixture-mac', platform: 'darwin', architecture: 'arm64' }, deployment: status });
    if (path === '/api/deployment') return Response.json(status);
    if (path === '/api/deployment/retry') { retries++; status = snapshot(0, 'preparing'); return Response.json({ deployment: status }); }
    if (path === '/api/deployment/resume') return request.json().then(input => {
        assert.deepEqual(input, { revision: 'fixture-revision' });
        resumes++; status = snapshot(0, 'preparing'); return Response.json({ deployment: status });
    });
    return new Response('', { status: 404 });
} });
let browser;
try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage({ viewport: { width: 1280, height: 1100 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    async function assertHorizontalStages() {
        const nodes = await page.locator('.deployment-stage-number').evaluateAll(items => items.map(item => {
            const rect = item.getBoundingClientRect();
            return { x: rect.x, y: rect.y };
        }));
        assert.equal(nodes.length, 6);
        assert.ok(nodes.every((node, index) => Math.abs(node.y - nodes[0].y) <= 1 && (!index || node.x > nodes[index - 1].x)), 'Six stages remain in one horizontal row');
    }
    await page.goto(server.url.href);
    await page.getByRole('heading', { name: /阶段 1 \/ 6/ }).waitFor();
    await assertHorizontalStages();
    assert.deepEqual(await page.locator('.deployment-stage-copy strong').allTextContents(), labels);
    assert.equal(await page.locator('#deployment-log-details').evaluate(node => node.open), false);
    assert.equal(await page.locator('#deployment-stages [data-state=done]').count(), 0);
    for (let index = 1; index < ids.length; index++) {
        status = snapshot(index);
        await page.waitForFunction(id => document.querySelector('#deployment-stages [aria-current=step]')?.getAttribute('data-stage') === id, ids[index]);
        assert.equal(await page.locator('#deployment-stages [data-state=done]').count(), index);
        assert.equal(await page.locator('#deployment-stages [data-state=active]').count(), 1);
        assert.equal(await page.locator('#deployment-stages script').count(), 0);
        if (index === 3) {
            await page.reload();
            await page.getByRole('heading', { name: /阶段 4 \/ 6/ }).waitFor();
            assert.equal(await page.locator('#deployment-stages [data-state=done]').count(), 3);
            status.stages[3].detail = status.message = '正在导入 Apeiron 后端镜像，校验镜像层和摘要。';
            await page.getByText(status.message, { exact: true }).first().waitFor();
            await page.screenshot({ path: join(directory, 'stages-import.png'), fullPage: true });
        }
    }
    status = snapshot(3, 'failed'); status.stages[3].state = 'failed'; status.message = '资源校验失败，请检查安装包。';
    await page.getByRole('heading', { name: '部署失败。', exact: true }).waitFor();
    assert.equal(await page.locator('[data-stage=import]').getAttribute('data-state'), 'failed');
    assert.equal(await page.locator('[data-stage=applications]').getAttribute('data-state'), 'pending');
    assert.equal(await page.locator('[data-stage=access]').getAttribute('data-state'), 'pending');
    await page.screenshot({ path: join(directory, 'stages-failed.png'), fullPage: true });
    await page.getByRole('button', { name: '重新部署', exact: true }).click();
    await page.getByRole('heading', { name: /阶段 1 \/ 6/ }).waitFor();
    assert.equal(retries, 1);
    assert.equal(await page.locator('#deployment-stages [data-state=done]').count(), 0);
    status = snapshot(2, 'cancelled'); status.stages[2].state = 'cancelled';
    await page.getByRole('heading', { name: '部署已停止。', exact: true }).waitFor();
    assert.equal(await page.locator('[data-stage=nexus]').getAttribute('data-state'), 'cancelled');
    assert.equal(await page.locator('[data-stage=import]').getAttribute('data-state'), 'pending');
    await page.setViewportSize({ width: 390, height: 844 });
    await assertHorizontalStages();
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.screenshot({ path: join(directory, 'stages-mobile.png'), fullPage: true });
    status = { phase: 'interrupted', resumable: true, events: [], message: '原部署等待身份核对后继续。' };
    await page.reload();
    await page.getByRole('heading', { name: '部署等待继续。', exact: true }).waitFor();
    assert.equal(await page.locator('#deployment-stages [data-state=done]').count(), 0);
    assert.match(await page.locator('#deployment-note').innerText(), /未完成引导的安装不会自动接管/);
    await page.getByRole('button', { name: '继续原部署', exact: true }).click();
    await page.getByRole('heading', { name: /阶段 1 \/ 6/ }).waitFor();
    assert.equal(resumes, 1);
    assert.equal(retries, 1);
    await page.reload();
    assert.equal(resumes, 1, 'Reload never starts another continuation');
    status = snapshot(5, 'succeeded'); status.stages.forEach(stage => { stage.state = 'done'; });
    await page.reload();
    await page.getByRole('heading', { name: '部署阶段已完成', exact: true }).waitFor();
    assert.equal(await page.locator('#deployment-stages [data-state=done]').count(), 6);
    assert.match(await page.locator('#deployment-stage-note').innerText(), /测试登录/);
    assert.deepEqual(errors, []);
    console.log(`PASS six deployment stages: live transitions, reload, failure, retry reset, cancellation, interrupted-run continuation, completion, safe text, mobile; screenshots: ${directory}`);
} finally {
    await browser?.close();
    server.stop(true);
}
