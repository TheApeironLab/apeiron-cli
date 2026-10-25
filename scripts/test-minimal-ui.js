// secrets: none
// C-DEP-63 minimal wizard @gate @cap-DEP; deterministic fixtures, no model calls.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { startInitServer } from '../test/rust-server';
import { deploymentFixture, freshInstallFixture } from '../test/fixtures';

const dir = await mkdtemp(join(tmpdir(), 'apeiron-minimal-ui-'));
const setup = await deploymentFixture(dir);
await freshInstallFixture(dir, setup);
await mkdir(join(dir, 'home'));
const catalogPath = join(setup.root, 'setup/install.json');
const catalog = JSON.parse(await readFile(catalogPath, 'utf8'));
catalog.distro = 'minimal';
catalog.applications = catalog.applications.filter(app => ['nexus', 'vasi', 'apeiron'].includes(app.id));
for (const target of Object.values(catalog.targets)) target.environment.distro = 'minimal';
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
    await page.getByRole('radio', { name: /Docker 中运行 K3d/ }).check();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('heading', { name: '选择发行版', exact: true }).waitFor();
    assert.equal(await page.getByRole('radio', { name: /^Minimal/ }).isChecked(), true);
    assert.equal(await page.getByRole('button', { name: '仅必选', exact: true }).count(), 0);
    assert.equal(await page.getByRole('button', { name: '核心协作', exact: true }).count(), 0);
    assert.equal(await page.getByRole('checkbox', { name: /Nexus|Vasi|Apeiron|Gateway/ }).count(), 0);
    assert.equal(await page.getByRole('list', { name: '发行版组件' }).count(), 0);
    assert.equal(await page.getByRole('radio', { name: 'Minimal 基础平台与 AI 能力。', exact: true }).isChecked(), true);
    await page.screenshot({ path: '/tmp/apeiron-distro-minimal.png', fullPage: true });
    // A stale base package cannot be relabelled as Minimal by the browser.
    catalog.distro = 'base';
    await writeFile(catalogPath, JSON.stringify(catalog));
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByText(/所选发行版与安装包不匹配/).waitFor();
    assert.equal(await page.getByRole('heading', { name: '选择发行版', exact: true }).isVisible(), true);
    catalog.distro = 'minimal';
    await writeFile(catalogPath, JSON.stringify(catalog));
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    assert.match(await page.getByRole('main').innerText(), /Apeiron 直接连接模型服务/);
    await page.getByRole('textbox', { name: 'Slug name', exact: true }).fill('minimal-test');
    await page.getByRole('textbox', { name: 'Base URL', exact: true }).fill('https://models.example.internal/v1');
    await page.getByLabel(/^API Key/).fill('fixture-only-minimal-key');
    await page.getByRole('button', { name: '获取模型列表', exact: true }).click();
    await page.getByRole('combobox', { name: '快速 · apeiron-flash' }).selectOption('fixture-chat');
    await page.getByRole('combobox', { name: '深度思考 · apeiron-pro' }).selectOption('fixture-chat');
    await page.getByRole('button', { name: '测试两种模式', exact: true }).click();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('heading', { name: '预检与部署摘要', exact: true }).waitFor();
    assert.equal((await page.getByRole('main').innerText()).includes('fixture-only-minimal-key'), false);
    assert.equal(preflightInput.deployment.installation.distro, 'minimal');
    assert.deepEqual(preflightInput.apps.sort(), ['apeiron', 'nexus', 'vasi']);
    // New packages describe every native release; the app label changes only entry visibility.
    const names = ['cert-manager', 'postgres', 'seaweedfs', 'keycloak', 'cluster-access', 'nexus', 'vasi', 'apeiron'];
    catalog.releaseCatalog = { schemaVersion: 1, releases: names.map(name => ({
        name, namespace: name, component: name === 'cert-manager' ? 'ingress' : name,
        required: true, needs: [], labels: { catalog: 'release-v1',
            ...(['nexus', 'vasi', 'apeiron'].includes(name) ? { app: 'true' } : {}) }
    })) };
    catalog.applications = catalog.releaseCatalog.releases.map(r => ({
        id: r.name, name: r.name, description: '', releases: [r.name], requires: [], required: true,
        app: r.labels.app === 'true'
    }));
    catalog.resourceBudget = { basis: 'helm-template', workloads: 8, memoryRequestsBytes: 0,
        memoryRequestsBytesPerNode: 0, missingMemoryRequests: ['postgres/StatefulSet/postgres/postgres'], transientWorkloads: [] };
    await writeFile(catalogPath, JSON.stringify(catalog));
    await page.goto(server.url);
    await page.getByRole('button', { name: '直接配置当前实例（不新建）' }).click();
    await page.getByRole('radio', { name: /Docker 中运行 K3d/ }).check();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByRole('button', { name: '下一步', exact: true }).click();
    await page.getByText('8 个 release，其中 3 个应用入口：nexus、vasi、apeiron', { exact: true }).waitFor();
    await page.getByText('查看全部 release', { exact: true }).click();
    assert.equal(await page.getByRole('list', { name: '发行版 release' }).getByRole('listitem').count(), 8);
    await page.getByText('Chart 未完整声明 memory requests（1 个容器缺失），暂不提供总内存估算。', { exact: true }).waitFor();
    assert.equal((await page.getByRole('main').innerText()).includes('7.5 GiB'), false);
    console.log('PASS C-DEP-63 minimal wizard @gate @cap-DEP: Minimal distro selection, concise description, rejects base package, direct models, redacted summary');
} finally {
    await browser.close();
    await server.stop();
    await rm(dir, { recursive: true, force: true });
}
