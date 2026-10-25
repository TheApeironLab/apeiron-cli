// Deterministic protocol fixture: exercises real scode without calling a model.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

export async function smokeSudocode(binary) {
  const root = await mkdtemp(join(tmpdir(), 'apeiron-sudo-native-'));
  let server;
  let child;
  let timer;
  try {
    const config = join(root, '.apeiron/sudo');
    const cwd = join(root, 'project');
    await mkdir(config, { recursive: true });
    await mkdir(cwd);
    const observed = [];
    server = Bun.serve({ hostname: '127.0.0.1', port: 0, async fetch(request) {
      if (request.method === 'GET' && new URL(request.url).pathname === '/v1/models')
        return Response.json({ object: 'list', data: [{ id: 'fixture', object: 'model' }] });
      const body = await request.json();
      observed.push({ path: new URL(request.url).pathname, model: body.model,
        authenticated: request.headers.get('authorization') === 'Bearer fixture-key',
        catalog: JSON.stringify(body).includes('Available CLI packages'),
        toolResult: body.messages?.filter(m => m.role === 'tool').map(m => m.content).join('\n') || '' });
      const bash = body.tools?.find(t => t.function?.name.toLowerCase() === 'bash');
      const callTool = observed.length === 1 && bash;
      const message = callTool
        ? { role: 'assistant', content: null, tool_calls: [{ id: 'call_apeiron', type: 'function',
            function: { name: bash.function.name, arguments: JSON.stringify({ command: 'apeiron --version', description: 'Read Apeiron version' }) } }] }
        : { role: 'assistant', content: 'minimal-direct-ok' };
      const finish = callTool ? 'tool_calls' : 'stop';
      const usage = { prompt_tokens: 10, completion_tokens: 4, total_tokens: 14 };
      if (!body.stream) return Response.json({ id: 'fixture', object: 'chat.completion', created: 1,
        model: 'fixture', choices: [{ index: 0, message, finish_reason: finish }], usage });
      const delta = callTool ? { ...message, tool_calls: message.tool_calls.map(t => ({ index: 0, ...t })) } : message;
      const chunks = [
        { id: 'fixture', object: 'chat.completion.chunk', created: 1, model: 'fixture', choices: [{ index: 0, delta, finish_reason: null }] },
        { id: 'fixture', object: 'chat.completion.chunk', created: 1, model: 'fixture', choices: [{ index: 0, delta: {}, finish_reason: finish }], usage },
      ];
      return new Response(chunks.map(c => 'data: ' + JSON.stringify(c) + '\n\n').join('') + 'data: [DONE]\n\n',
        { headers: { 'Content-Type': 'text/event-stream' } });
    } });
    await writeFile(join(config, 'sudocode.json'), JSON.stringify({
      auth_modes: { 'api-key': { openai: { baseUrl: `http://127.0.0.1:${server.port}/v1`, apiKeyEnv: 'APEIRON_MODEL_API_KEY' } } },
      models: { minimal: { alias: 'minimal', name: 'Fixture', providers: {
        'api-key': { provider: 'openai', model: 'fixture', api: 'openai-completions' },
      } } },
    }));
    child = Bun.spawn([binary, 'sudo', '--auth', 'api-key', '--model', 'minimal', '--allowedTools', 'Bash',
      '-p', 'Read the Apeiron version', '--output-format', 'json'], {
      cwd, env: { HOME: root, PATH: '/usr/bin:/bin', APEIRON_MODEL_API_KEY: 'fixture-key' },
      stdin: 'ignore', stdout: 'pipe', stderr: 'pipe',
    });
    timer = setTimeout(() => child.kill('SIGKILL'), 30_000);
    const [stdout, stderr, code] = await Promise.all([
      new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
    ]);
    assert.equal(code, 0, stderr || stdout);
    const result = JSON.parse(stdout);
    assert.equal(result.is_error, false);
    assert.equal(result.result, 'minimal-direct-ok');
    assert.equal(observed.length, 2, 'Expected a real shell tool round trip');
    assert.ok(observed.every(r => r.path === '/v1/chat/completions' && r.model === 'fixture' && r.authenticated));
    assert.ok(observed[0].catalog, 'Apeiron CLI package must be visible to the agent');
    const version = Bun.spawnSync([binary, '--version']).stdout.toString().trim();
    assert.ok(observed[1].toolResult.includes(version), 'Agent must execute the bundled Apeiron CLI');
    console.log('PASS Sudocode: embedded extraction, direct provider, JSON result and agent → apeiron tool call');
  } finally {
    clearTimeout(timer);
    if (child && child.exitCode === null) { child.kill('SIGKILL'); await child.exited; }
    server?.stop(true);
    await rm(root, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  if (!process.argv[2]) throw new Error('Usage: bun scripts/smoke-sudo.js /path/to/apeiron');
  await smokeSudocode(resolve(process.argv[2]));
}
