import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { deploymentFixture } from '../test/fixtures';

const dir = await mkdtemp(join(tmpdir(), 'apeiron-wizard-ui-'));
const path = join(dir, 'config.json');
const setup = await deploymentFixture(dir);
await writeFile(setup.environment, setup.source.replace('fixtureExit: 0', 'fixtureExit: 17').replace('fixtureDelay: 700', 'fixtureDelay: 1500'));
const child = Bun.spawn([resolve(import.meta.dir, '../dist/apeiron'), 'init', '--no-open', '--config', path], {
  cwd: dir, stdout: 'pipe', stderr: 'pipe', env: { ...process.env, PATH: setup.bin + ':' + process.env.PATH },
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
  await page.goto(url);
  assert.equal(await page.getByLabel('Base URL', { exact: true }).count(), 0);
  const slug = page.getByLabel('Slug name', { exact: true });
  await slug.fill('Bad Name');
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await slug.isVisible(), true);
  await slug.fill('example-team');
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await slug.isVisible(), true, 'Missing deployment target must not advance');
  await page.getByLabel('部署方式', { exact: true }).selectOption('docker');
  await page.getByLabel('宸途仓库路径', { exact: true }).fill(setup.root);
  await page.getByLabel('环境 values 文件', { exact: true }).fill(setup.environment);
  await page.getByLabel('工作目录', { exact: true }).fill(setup.workDir);
  await page.getByLabel('工具箱镜像', { exact: true }).fill('fixture-toolbox');
  await page.screenshot({ path: join(dir, 'step-1.png'), fullPage: true });
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await page.getByRole('heading', { name: '选择要启用的应用', exact: true }).waitFor();
  const required = ['Vasi', 'Apeiron', 'Limani', 'Task', 'Corpus', 'Chat', 'Nexus', '邮件'];
  const defaults = ['Files', 'Gateway'];
  const optional = ['Filer', '代码仓库', 'GPUStack', 'Langfuse', 'Grafana'];
  const app = (name: string) => page.getByRole('checkbox', { name: new RegExp('^' + name + ' ') });
  assert.deepEqual(await page.locator('.app-copy strong').allTextContents(), ['Vasi', 'Apeiron', 'Limani', 'Task', 'Corpus', 'Chat', 'Files', 'Gateway', 'Nexus', 'Filer', '邮件', '代码仓库', 'GPUStack', 'Langfuse', 'Grafana']);
  for (const name of required) { assert.equal(await app(name).isChecked(), true); assert.equal(await app(name).isDisabled(), true); }
  for (const name of [...defaults, ...optional]) {
    assert.equal(await app(name).isChecked(), defaults.includes(name));
    assert.equal(await app(name).isEnabled(), true);
  }
  assert.equal(await page.locator('#selected-count').textContent(), '已选择 10 个应用（8 个必选）');
  await page.screenshot({ path: join(dir, 'step-2.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: join(dir, 'mobile-apps.png'), fullPage: true });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.setViewportSize({ width: 1280, height: 920 });
  for (const name of defaults) await app(name).uncheck();
  for (const name of optional) await app(name).check();
  await page.getByRole('button', { name: '上一步', exact: true }).click();
  assert.equal(await page.getByLabel('环境 values 文件', { exact: true }).inputValue(), setup.environment);
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await app('Grafana').isChecked(), true);
  assert.equal(await app('Files').isChecked(), false);
  await page.getByRole('button', { name: '开始部署', exact: true }).click();
  await page.getByRole('heading', { name: '正在部署…', exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: '完成并关闭向导' }).isDisabled(), true);
  await page.reload();
  await page.getByRole('heading', { name: '部署失败。', exact: true }).waitFor();
  assert.equal((await readFile(setup.calls, 'utf8')).trim().split('\n').length, 1, 'Reload does not start another deployment');
  assert.equal((await page.locator('body').textContent())!.includes('private-log-test-key'), false);
  await page.screenshot({ path: join(dir, 'deployment-failed.png'), fullPage: true });
  const saved = JSON.parse(await readFile(path, 'utf8'));
  assert.equal(saved.schemaVersion, 2);
  assert.equal(saved.slug, 'example-team');
  assert.equal(saved.llm, undefined);
  assert.deepEqual(saved.apps, ['vasi', 'apeiron', 'ontology', 'task', 'corpus', 'matrix', 'nexus', 'filer', 'stalwart', 'git', 'gpustack', 'langfuse', 'kps']);
  assert.equal((await stat(path)).mode & 0o777, 0o600);
  await page.getByRole('button', { name: '返回修改' }).click();
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  for (const name of [...required, ...optional]) assert.equal(await app(name).isChecked(), true);
  for (const name of defaults) assert.equal(await app(name).isChecked(), false);
  await writeFile(setup.environment, setup.source);
  await page.getByRole('button', { name: '开始部署', exact: true }).click();
  await page.getByRole('heading', { name: '部署完成。', exact: true }).waitFor();
  assert.equal((await readFile(setup.calls, 'utf8')).trim().split('\n').length, 2);
  await page.screenshot({ path: join(dir, 'deployment-complete.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: join(dir, 'mobile-deployment.png'), fullPage: true });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.getByRole('button', { name: '完成并关闭向导', exact: true }).click();
  await page.getByRole('button', { name: '向导已关闭', exact: true }).waitFor();
  assert.equal(await child.exited, 0);
  assert.deepEqual(errors, []); assert.deepEqual(externalRequests, []);
  let output = startup;
  const remaining = child.stdout.getReader();
  for (;;) { const chunk = await remaining.read(); if (chunk.done) break; output += new TextDecoder().decode(chunk.value); }
  remaining.releaseLock(); output += await new Response(child.stderr).text();
  assert.equal(output.includes('private-log-test-key'), false);
  console.log(`PASS compiled CLI + fixture deployer: two-step setup, selections, failure/retry, reload, finish, desktop/mobile\nscreenshots: ${dir}`);
} finally {
  clearTimeout(timeout); await browser?.close(); child.kill(); await child.exited;
  // Preserve screenshots only; fixture files and logs contain no real credentials.
  for (const entry of ['config.json', 'deployments', 'chentu fixture', 'work', 'bin', 'base.yaml', 'calls.jsonl', 'fixture.ts']) await rm(join(dir, entry), { recursive: true, force: true });
}
