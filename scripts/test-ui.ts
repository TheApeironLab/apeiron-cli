import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
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
  await page.getByRole('checkbox', { name: /^Apeiron / }).uncheck();
  await page.getByRole('checkbox', { name: /Limani/ }).uncheck();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await page.getByRole('alert').textContent(), '请至少选择一个应用。');
  assert.equal(await page.getByLabel('Base URL', { exact: true }).isVisible(), false);
  await page.getByRole('checkbox', { name: /^Apeiron / }).check();
  await page.getByRole('checkbox', { name: /Limani/ }).check();
  await page.getByRole('checkbox', { name: /Corpus/ }).check();
  await page.screenshot({ path: join(dir, 'step-2.png'), fullPage: true });
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('heading', { name: '连接你的模型', exact: true }).waitFor();
  await page.getByRole('button', { name: '保存配置', exact: true }).click();
  assert.equal(await page.getByLabel('Base URL', { exact: true }).isVisible(), true, 'Empty model config must not save');
  await page.getByLabel('Base URL', { exact: true }).fill('https://models.example.internal/v1');
  await page.getByLabel(/^API Key /).fill('test-only-browser-key');
  await page.getByLabel('Model ID', { exact: true }).fill('example-model');
  await page.getByRole('button', { name: '上一步', exact: true }).click();
  assert.equal(await page.getByRole('checkbox', { name: /Corpus/ }).isChecked(), true, 'App selection survives back navigation');
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
  assert.deepEqual(saved.apps, ['apeiron', 'ontology', 'corpus']);
  assert.equal((await stat(path)).mode & 0o777, 0o600);
  // Reload exercises redacted initial state and retaining an existing key.
  await page.reload();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await page.getByRole('checkbox', { name: /Corpus/ }).isChecked(), true);
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await page.getByLabel(/^API Key /).inputValue(), '');
  await page.getByLabel('Model ID', { exact: true }).fill('updated-model');
  await page.getByRole('button', { name: '保存配置', exact: true }).click();
  await page.getByRole('heading', { name: '准备好了。' }).waitFor();
  assert.equal(JSON.parse(await readFile(path, 'utf8')).llm.apiKey, 'test-only-browser-key');
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
