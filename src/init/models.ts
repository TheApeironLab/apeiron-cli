import { spawn } from 'node:child_process';
import { join } from 'node:path';
import { ConfigError, type DeploymentTarget } from './config';

export interface ModelConfiguration { provider: string; baseUrl: string; apiKey: string; fast: string; deep: string }
export function modelConfiguration(value: unknown, current?: ModelConfiguration): ModelConfiguration {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new ConfigError('模型配置格式不正确。');
  const v = value as Record<string, unknown>;
  const field = (key: string, max: number) => {
    const x = v[key];
    if (typeof x !== 'string' || x.length > max || /[\x00-\x1f\x7f]/.test(x)) throw new ConfigError('模型配置字段格式不正确。');
    return x.trim();
  };
  const provider = field('provider', 64);
  if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(provider)) throw new ConfigError('请填写 Provider：小写字母、数字、点、下划线或短横线，以字母或数字开头。');
  const baseUrl = field('baseUrl', 2048).replace(/\/+$/, '');
  let url: URL;
  try { url = new URL(baseUrl); } catch { throw new ConfigError('请填写有效的模型 API 地址。'); }
  if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new ConfigError('模型 API 地址只支持 HTTP/HTTPS，不能包含凭据、查询参数或片段。');
  const apiKey = v.apiKey === undefined && current?.baseUrl === baseUrl ? current.apiKey : field('apiKey', 4096);
  const fast = field('fast', 256), deep = field('deep', 256);
  if (!fast || !deep) throw new ConfigError('请选择快速和深度思考模型。');
  return { provider, baseUrl, apiKey, fast, deep };
}

export function modelValues(model: ModelConfiguration) {
  return {
    providers: [{ id: model.provider, baseUrl: model.baseUrl, apiKeyEnv: 'MODEL_API_KEY_SETUP' }],
    models: [
      { id: 'apeiron-flash', provider: model.provider, model: model.fast, label: 'apeiron-flash' },
      { id: 'apeiron-pro', provider: model.provider, model: model.deep, label: 'apeiron-pro' },
    ],
    modes: { fast: 'apeiron-flash', deep: 'apeiron-pro' },
    queues: [...new Set([model.fast, model.deep])].map(name => ({ provider: model.provider, model: name, maxConcurrency: 4, maxPending: 16 })),
  };
}

// Never forward redirects or provider error bodies (they may echo credentials).
export async function modelRequest(model: ModelConfiguration, action: 'list' | 'test', signal: AbortSignal, request = fetch) {
  const call = async (path: string, body?: object) => {
    const response = await request(`${model.baseUrl}/${path}`, { method: body ? 'POST' : 'GET', redirect: 'error',
      headers: { 'Content-Type': 'application/json', ...(model.apiKey ? { Authorization: `Bearer ${model.apiKey}` } : {}) },
      ...(body ? { body: JSON.stringify(body) } : {}), signal: AbortSignal.any([signal, AbortSignal.timeout(45_000)]) });
    if (!response.ok) { await response.body?.cancel(); throw new ConfigError(`模型服务返回 HTTP ${response.status}，请检查地址、密钥和模型权限。`); }
    const reader = response.body?.getReader(); if (!reader) throw new Error();
    const chunks: Uint8Array[] = []; let size = 0;
    try { for (;;) { const part = await reader.read(); if (part.done) break; size += part.value.length; if (size > 1_048_576) throw new Error(); chunks.push(part.value); } }
    finally { await reader.cancel(); }
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  };
  try {
    if (action === 'list') {
      const data = await call('models');
      if (!Array.isArray(data.data)) throw new Error();
      const models = [...new Set<string>(data.data.map((m: any) => m?.id).filter((id: unknown): id is string => typeof id === 'string' && id.length > 0 && id.length <= 256 && !/[\x00-\x1f\x7f]/.test(id)))].slice(0, 500);
      if (!models.length) throw new ConfigError('模型服务未返回可用模型，请检查 API 地址、Key 和模型权限。');
      return { models };
    }
    const results = [];
    for (const mode of ['fast', 'deep'] as const) {
      const started = Date.now();
      const data = await call('chat/completions', { model: model[mode], messages: [{ role: 'user', content: 'Reply OK.' }], max_tokens: 64, stream: false });
      if (!Array.isArray(data.choices) || !data.choices.length || !data.choices[0]?.message || !(data.choices[0].message.content || data.choices[0].message.reasoning_content)) throw new ConfigError('模型返回了空响应或不兼容的响应，请检查模型接口。');
      results.push({ mode, model: model[mode], elapsedMs: Date.now() - started });
    }
    return { results };
  } catch (error) {
    if (error instanceof ConfigError) throw error;
    throw new ConfigError('无法完成模型请求：请检查 CLI 主机的网络、服务地址和接口兼容性（45 秒超时）。');
  }
}

// JSON travels only over stdin. Do not log subprocess output or command errors:
// kubectl diagnostics can contain the submitted Secret. Merge only our key.
export async function installModelKey(model: ModelConfiguration, target: DeploymentTarget, env: NodeJS.ProcessEnv, signal: AbortSignal) {
  const manifests = [
    { apiVersion: 'v1', kind: 'Namespace', metadata: { name: 'apeiron' } },
    { apiVersion: 'v1', kind: 'Secret', metadata: { name: 'apeiron-model-keys', namespace: 'apeiron' }, type: 'Opaque',
      data: { APEIRON_MODEL_API_KEY_SETUP: Buffer.from(model.apiKey || 'not-required').toString('base64') } },
    // Cloud workbenches consume this Secret directly via envFrom. The backend
    // key alone is insufficient on releases whose lifecycle does not copy it.
    { apiVersion: 'v1', kind: 'Secret', metadata: { name: 'apeiron-scode-creds', namespace: 'apeiron' }, type: 'Opaque',
      data: { MODEL_API_KEY_SETUP: Buffer.from(model.apiKey || 'not-required').toString('base64') } },
  ];
  const kubectlArgs = ['--kubeconfig', target.runner === 'native' ? target.kubeconfig : '/kubeconfig', 'apply', '--server-side', '--field-manager=apeiron-setup-model', '-f', '-'];
  const command = target.runner === 'native' ? 'kubectl' : 'docker';
  const args = target.runner === 'native' ? kubectlArgs : ['run', '--rm', '-i', '--pull=never', '--name', env.LAB_CONTAINER!,
    '--network', env.LAB_NETWORK || `k3d-${env.LAB_CLUSTER || 'chentu-helmfile'}`, '--entrypoint', 'kubectl',
    '-v', `${join(target.workDir, 'state/kubeconfig')}:/kubeconfig:ro`, target.image, ...kubectlArgs];
  try {
    await new Promise<void>((resolve, reject) => {
      const child = spawn(command, args, { env, stdio: ['pipe', 'ignore', 'ignore'], signal, timeout: 30_000 });
      child.once('error', reject); child.stdin.on('error', reject);
      child.once('close', code => code === 0 ? resolve() : reject(new Error()));
      child.stdin.end(JSON.stringify({ apiVersion: 'v1', kind: 'List', items: manifests }));
    });
  } catch { throw new ConfigError('无法写入模型 Secret，已停止部署。请检查集群连接与权限。'); }
}
