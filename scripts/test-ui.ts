import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const dir = await mkdtemp(join(tmpdir(), 'apeiron-wizard-ui-'));
const path = join(dir, 'config.json');
const binary = resolve(import.meta.dir, '../dist/apeiron');
const child = Bun.spawn([binary, 'init', '--no-open', '--config', path], { cwd: dir, stdout: 'pipe', stderr: 'pipe' });
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
  await page.goto(url);
  const slug = page.getByLabel('Slug name', { exact: true });
  await slug.fill('Bad Name');
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await slug.isVisible(), true, 'Invalid slug must not advance');
  await slug.fill('example-team');
  await page.screenshot({ path: join(dir, 'step-1.png'), fullPage: true });
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('heading', { name: '选择要启用的应用', exact: true }).waitFor();
  const required = ['Vasi', 'Apeiron', 'Limani', 'Task', 'Corpus', 'Chat'];
  const defaults = ['Files', 'Gateway', 'Nexus'];
  const optional = ['Filer', '邮件', '代码仓库', 'GPUStack', 'Langfuse', 'Grafana'];
  const app = (name: string) => page.getByRole('checkbox', { name: new RegExp('^' + name + ' ') });
  assert.deepEqual(await page.locator('.app-copy strong').allTextContents(), [...required, ...defaults, ...optional]);
  for (const name of required) {
    assert.equal(await app(name).isChecked(), true, `${name} is required`);
    assert.equal(await app(name).isDisabled(), true, `${name} cannot be deselected`);
  }
  for (const name of [...defaults, ...optional]) {
    assert.equal(await app(name).isChecked(), defaults.includes(name), `${name} default selection`);
    assert.equal(await app(name).isEnabled(), true, `${name} can be changed`);
  }
  assert.equal(await page.locator('#selected-count').textContent(), '已选择 9 个应用（6 个必选）');
  await page.screenshot({ path: join(dir, 'step-2.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: join(dir, 'mobile-apps.png'), fullPage: true });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, 'Mobile app list must not overflow');
  await page.setViewportSize({ width: 1280, height: 920 });
  for (const name of defaults) await app(name).uncheck();
  for (const name of optional) await app(name).check();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('heading', { name: '连接你的模型', exact: true }).waitFor();
  await page.getByRole('button', { name: '保存配置', exact: true }).click();
  assert.equal(await page.getByLabel('Base URL', { exact: true }).isVisible(), true, 'Empty model config must not save');
  await page.getByLabel('Base URL', { exact: true }).fill('https://models.example.internal/v1');
  await page.getByLabel(/^API Key /).fill('test-only-browser-key');
  await page.getByLabel('Model ID', { exact: true }).fill('example-model');
  await page.getByRole('button', { name: '上一步', exact: true }).click();
  assert.equal(await app('Grafana').isChecked(), true, 'App selection survives back navigation');
  assert.equal(await app('Files').isChecked(), false, 'Deselection survives back navigation');
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await page.getByLabel('Model ID', { exact: true }).inputValue(), 'example-model', 'Model input survives back navigation');
  await page.screenshot({ path: join(dir, 'step-3.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: join(dir, 'mobile.png'), fullPage: true });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, 'Mobile page must not overflow');
  await page.getByRole('button', { name: '保存配置', exact: true }).click();
  await page.getByRole('heading', { name: '准备好了。' }).waitFor();
  const saved = JSON.parse(await readFile(path, 'utf8'));
  assert.equal(saved.slug, 'example-team');
  assert.equal(saved.llm.apiKey, 'test-only-browser-key');
  const selectedIds = ['vasi', 'apeiron', 'ontology', 'task', 'corpus', 'matrix', 'filer', 'stalwart', 'git', 'gpustack', 'langfuse', 'kps'];
  assert.deepEqual(saved.apps, selectedIds);
  assert.equal((await stat(path)).mode & 0o777, 0o600);
  // Reload exercises redacted initial state and retaining an existing key.
  await page.reload();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  for (const name of [...required, ...optional]) assert.equal(await app(name).isChecked(), true);
  for (const name of defaults) assert.equal(await app(name).isChecked(), false, 'Saved optional choices override fresh defaults');
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await page.getByLabel(/^API Key /).inputValue(), '');
  await page.getByLabel('Model ID', { exact: true }).fill('updated-model');
  await page.getByRole('button', { name: '保存配置', exact: true }).click();
  await page.getByRole('heading', { name: '准备好了。' }).waitFor();
  assert.equal(JSON.parse(await readFile(path, 'utf8')).llm.apiKey, 'test-only-browser-key');
  // Opening an older config adds required choices in the form, but does not write until Save.
  const legacy = JSON.stringify({ ...saved, apps: ['apeiron', 'ontology', 'filer'] });
  await writeFile(path, legacy);
  await page.reload();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  for (const name of required) {
    assert.equal(await app(name).isChecked(), true);
    assert.equal(await app(name).isDisabled(), true);
  }
  assert.equal(await app('Filer').isChecked(), true);
  for (const name of [...defaults, ...optional.filter(name => name !== 'Filer')]) assert.equal(await app(name).isChecked(), false);
  assert.equal(await readFile(path, 'utf8'), legacy);
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('button', { name: '保存配置', exact: true }).click();
  await page.getByRole('heading', { name: '准备好了。' }).waitFor();
  assert.deepEqual(JSON.parse(await readFile(path, 'utf8')).apps, ['vasi', 'apeiron', 'ontology', 'task', 'corpus', 'matrix', 'filer']);
  await page.getByRole('button', { name: '完成并关闭向导', exact: true }).click();
  await page.getByRole('button', { name: '向导已关闭', exact: true }).waitFor();
  assert.equal(await child.exited, 0);
  assert.deepEqual(errors, []);
  assert.deepEqual(externalRequests, []);
  let output = startup;
  const remaining = child.stdout.getReader();
  for (;;) {
    const chunk = await remaining.read();
    if (chunk.done) break;
    output += new TextDecoder().decode(chunk.value);
  }
  remaining.releaseLock();
  output += await new Response(child.stderr).text();
  assert.equal(output.includes('test-only-browser-key'), false);
  console.log(`PASS compiled CLI: create/edit/retain key/finish, desktop + mobile, no external requests\nscreenshots: ${dir}`);
} finally {
  clearTimeout(timeout);
  await browser?.close();
  child.kill();
  await child.exited;
  await rm(path, { force: true });
}
