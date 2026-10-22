// Validate the public default package with real Helmfile, without a cluster or network in the container.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import APPS from '../runtime/assets/apps.json';
import { buildSupport } from './lib/rust';
import { fixtureModels } from '../test/fixtures';
const directory = await mkdtemp(join(tmpdir(), 'apeiron-install-plan-'));
async function run(args) {
    const child = Bun.spawn(args, { stdout: 'pipe', stderr: 'pipe' });
    const [out, err, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
    assert.equal(code, 0, `${args[0]} failed: ${err}`);
    return out;
}
async function download(url, path, digest, size) {
    assert.match(url, /^https:\/\//);
    await run(['curl', '--fail', '--silent', '--show-error', '--location', '--proto', '=https', '--proto-redir', '=https', '--retry', '3', '--max-time', '300', '--output', path, url]);
    const bytes = await readFile(path);
    assert.equal(createHash('sha256').update(bytes).digest('hex'), digest, url);
    if (size !== undefined) assert.equal(bytes.length, size, url);
}
try {
    assert.equal(process.argv.length === 2 || process.argv.length === 4, true, 'Usage: bun scripts/test-install-plan.js [CHENTU_ROOT TOOLBOX_IMAGE]');
    let root = process.argv[2] && resolve(process.argv[2]);
    let image = process.argv[3];
    const target = `k3d-${process.arch === 'x64' ? 'amd64' : process.arch}`;
    if (!root) {
        // Read the runtime's exact pin, so CI cannot silently validate a different package.
        const source = await readFile(new URL('../runtime/src/app/resources.rs', import.meta.url), 'utf8');
        const pin = name => {
            const value = new RegExp(`^const ${name}: &str = "([^"]+)";`, 'm').exec(source)?.[1];
            assert.ok(value, `Cannot read runtime ${name} pin`);
            return value;
        };
        const version = pin('VERSION');
        assert.match(version, /^[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$/);
        const archive = join(directory, 'chentu.tar.gz');
        await download(`https://${pin('HOST')}/chentu/releases/${version}/chentu-${version}.tar.gz`, archive, pin('DIGEST'));
        await run(['tar', '-xzf', archive, '-C', directory]);
        root = join(directory, `chentu-${version}`);
    }
    const required = APPS.filter(app => app.required).map(app => app.id);
    const basePlan = await buildSupport('plan', root, target, JSON.stringify(required));
    if (!image) {
        image = basePlan.target.toolboxImageId;
        assert.match(image, /^sha256:[a-f0-9]{64}$/);
        const existing = Bun.spawnSync(['docker', 'image', 'inspect', image], { stdout: 'ignore', stderr: 'ignore' });
        if (existing.exitCode !== 0) {
            const file = basePlan.files.find(file => file.path === basePlan.target.toolboxArchive);
            assert.ok(file, 'Default package must declare a verified toolbox archive');
            const archive = join(directory, 'toolbox.tar');
            await download(file.url, archive, file.sha256, file.size);
            await run(['docker', 'load', '--input', archive]);
        }
        assert.equal((await run(['docker', 'image', 'inspect', image, '--format', '{{.Id}}'])).trim(), image);
    }
    for (const extra of [[], ['task'], ['langfuse'], ['kps']]) {
        const apps = [...required, ...extra];
        const plan = await buildSupport('plan', root, target, JSON.stringify(apps));
        const replacements = { __DOMAIN__: 'example.internal', __REGISTRY__: 'registry.example.internal', __NODE__: 'fixture-node', __WORK__: '/work/state', __BUNDLE__: '/work/bundle', __ARCH__: target.slice(4) };
        let text = JSON.stringify(plan.environment);
        for (const [from, to] of Object.entries(replacements)) text = text.replaceAll(from, to);
        const config = { schemaVersion: 2, slug: 'integration-team', apps, models: fixtureModels,
            deployment: { offline: false, installation: { topology: 'single-k3d', domain: 'example.internal', entryIp: '127.0.0.1', httpPort: 54320, httpsPort: 54321 } } };
        await writeFile(join(directory, 'config.json'), JSON.stringify(config), { mode: 0o600 });
        await writeFile(join(directory, 'planned.json'), text, { mode: 0o600 });
        const environment = await buildSupport('environment', join(directory, 'config.json'), join(directory, 'planned.json'));
        await writeFile(join(directory, 'environment.json'), JSON.stringify(environment), { mode: 0o600 });
        const docker = ['docker', 'run', '--rm', '--pull', 'never', '--network', 'none',
            '-v', `${root}:/repo:ro`, '-v', `${directory}:/input:ro`, '-w', '/repo/deploy/helmfile',
            '-e', 'CHENTU_ENV=/input/environment.json', '-e', 'PYTHONPATH=/repo/cli/src:/repo/deploy/helmfile/scripts'];
        const resolved = Bun.YAML.parse(await run([...docker, '--entrypoint', 'helmfile', image,
            '--environment', 'single-k3d', '--file', 'helmfile.yaml.gotmpl', 'print-env'])).values;
        for (const app of APPS) {
            for (const name of app.releases ?? [app.id]) {
                assert.equal(resolved.releases[name]?.enabled, plan.components.includes(app.id) || plan.components.includes(name), name);
            }
        }
        await run([...docker, '--entrypoint', 'sh', image, '-c', 'exec "${CHENTU_PYTHON:-python3}" /repo/deploy/helmfile/scripts/check.py']);
        console.log(`PASS real default-package Helmfile/check.py: ${extra.join(', ') || 'four required apps only'}; no cluster or container network`);
    }
} finally {
    await rm(directory, { recursive: true, force: true });
}
