// Kept self-contained so the CLI can embed this function in the local page.
// Bun strips TypeScript; both source and compiled binary paths are browser-tested.
export function initWizard(): void {
  type App = { id: string; name: string; description: string; selected: boolean; required: boolean };
  type PublicConfig = { slug: string; apps: string[]; llm: { baseUrl: string; modelId: string; hasApiKey: boolean } };
  type Snapshot = { revision: string | null; config: PublicConfig | null; apps: App[]; path: string };
  const get = <T extends HTMLElement>(id: string): T => document.getElementById(id) as T;
  const form = get<HTMLFormElement>('setup-form');
  const slug = get<HTMLInputElement>('slug');
  const baseUrl = get<HTMLInputElement>('base-url');
  const apiKey = get<HTMLInputElement>('api-key');
  const modelId = get<HTMLInputElement>('model-id');
  const error = get('error');
  const next = get<HTMLButtonElement>('next');
  const back = get<HTMLButtonElement>('back');
  let step = 0;
  let busy = false;
  let revision: string | null = null;
  let apps: App[] = [];
  let hasApiKey = false;
  let path = '';
  const titles = ['给你的组织起个名字', '选择要启用的应用', '连接你的模型'];
  const subtitles = ['这个标识会用于你的 Apeiron 配置。', '基础应用已设为必选，其余应用可以按需调整。', '填写模型服务提供的连接信息。'];
  const endpoint = (name: string) => new URL('api/' + name, location.href).href;

  function showError(message: string) { error.textContent = message; error.hidden = false; }
  function renderStep(focus = true) {
    error.hidden = true;
    document.querySelectorAll<HTMLElement>('[data-panel]').forEach((panel, index) => { panel.hidden = index !== step; });
    document.querySelectorAll<HTMLElement>('[data-step]').forEach((item, index) => {
      item.dataset.state = index === step ? 'active' : index < step ? 'done' : 'pending';
      if (index === step) item.setAttribute('aria-current', 'step'); else item.removeAttribute('aria-current');
    });
    get('step-label').textContent = `第 ${step + 1} 步 / 共 3 步`;
    get('step-title').textContent = titles[step]!;
    get('step-subtitle').textContent = subtitles[step]!;
    back.hidden = step === 0;
    next.textContent = step === 2 ? '保存配置' : '下一步';
    if (focus) (step === 0 ? slug : step === 2 ? baseUrl : document.querySelector<HTMLInputElement>('[name="app"]:not(:disabled)'))?.focus();
  }

  function selectedApps(): string[] {
    return Array.from(document.querySelectorAll<HTMLInputElement>('[name="app"]:checked')).map(input => input.value);
  }

  function updateSelection() {
    get('selected-count').textContent = `已选择 ${selectedApps().length} 个应用（${apps.filter(app => app.required).length} 个必选）`;
  }

  function renderApps(selected: string[]) {
    const grid = get('app-grid');
    grid.replaceChildren();
    apps.forEach(app => {
      const label = document.createElement('label');
      label.className = 'app-card';
      label.dataset.required = String(app.required);
      const input = document.createElement('input');
      input.type = 'checkbox'; input.name = 'app'; input.value = app.id;
      // Newly required apps are selected when loading an older config too.
      // Optional selections retain the user's saved values.
      input.checked = app.required || selected.includes(app.id);
      input.disabled = app.required;
      input.addEventListener('change', updateSelection);
      const icon = document.createElement('span');
      icon.className = 'app-icon'; icon.textContent = app.name.slice(0, 1); icon.setAttribute('aria-hidden', 'true');
      const copy = document.createElement('span'); copy.className = 'app-copy';
      const name = document.createElement('strong'); name.textContent = app.name;
      const description = document.createElement('span'); description.textContent = app.description;
      copy.append(name, description); label.append(input, icon, copy); grid.append(label);
      if (app.required) {
        const badge = document.createElement('span'); badge.className = 'required-badge';
        badge.textContent = '必选'; label.append(badge);
      }
    });
    updateSelection();
  }

  function validate(): boolean {
    const fields = step === 0 ? [slug] : step === 2 ? [baseUrl, modelId, apiKey] : [];
    for (const field of fields) if (!field.reportValidity()) return false;
    if (step === 2) {
      try {
        const url = new URL(baseUrl.value.trim());
        if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error();
      } catch { showError('请填写 HTTP/HTTPS Base URL，不要包含密钥、查询参数或片段。'); return false; }
    }
    if (step === 1 && apps.some(app => app.required && !selectedApps().includes(app.id))) {
      showError('请保留全部必选应用。'); return false;
    }
    return true;
  }

  function setBusy(value: boolean) {
    busy = value;
    get<HTMLFieldSetElement>('fields').disabled = value;
    back.disabled = value; next.disabled = value;
  }

  function applyConfig(config: PublicConfig) {
    slug.value = config.slug;
    baseUrl.value = config.llm.baseUrl;
    modelId.value = config.llm.modelId;
    hasApiKey = config.llm.hasApiKey;
    apiKey.value = '';
    apiKey.type = 'password';
    get<HTMLButtonElement>('toggle-key').textContent = '显示';
    get('toggle-key').setAttribute('aria-label', '显示 API Key');
    apiKey.placeholder = hasApiKey ? '已保存，留空即可保留原密钥' : '输入 API Key';
    get<HTMLInputElement>('clear-key').checked = false;
    get('clear-key-row').hidden = !hasApiKey;
    renderApps(config.apps);
  }

  back.addEventListener('click', () => { if (!busy && step > 0) { step--; renderStep(); } });
  get('toggle-key').addEventListener('click', () => {
    const visible = apiKey.type === 'password';
    apiKey.type = visible ? 'text' : 'password';
    get('toggle-key').textContent = visible ? '隐藏' : '显示';
    get('toggle-key').setAttribute('aria-label', visible ? '隐藏 API Key' : '显示 API Key');
  });
  get('edit').addEventListener('click', () => {
    get('success').hidden = true; form.hidden = false; get('form-heading').hidden = false;
    step = 0; renderStep();
  });
  get('finish').addEventListener('click', async () => {
    const button = get<HTMLButtonElement>('finish'); button.disabled = true;
    try {
      const response = await fetch(endpoint('finish'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
      if (!response.ok) throw new Error();
      button.textContent = '向导已关闭'; get('edit').hidden = true;
      get('success-note').textContent = '本地配置服务已退出，你可以关闭此页面。';
    } catch { button.disabled = false; get('success-note').textContent = '无法连接本地服务，可在终端按 Ctrl+C 关闭。'; }
  });
  form.addEventListener('submit', async event => {
    event.preventDefault();
    if (busy || !validate()) return;
    if (step < 2) { step++; renderStep(); return; }
    setBusy(true); next.textContent = '正在保存…'; error.hidden = true;
    try {
      const key = apiKey.value.trim();
      const clear = get<HTMLInputElement>('clear-key').checked;
      const response = await fetch(endpoint('config'), {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision, slug: slug.value.trim(), apps: selectedApps(), llm: {
          baseUrl: baseUrl.value.trim(), modelId: modelId.value.trim(),
          ...(key || clear || !hasApiKey ? { apiKey: clear ? '' : key } : {}),
        } }),
      });
      const result = await response.json() as Snapshot & { error?: string };
      if (!response.ok || !result.config) throw new Error(result.error || '保存失败，请稍后重试。');
      revision = result.revision; applyConfig(result.config);
      form.hidden = true; get('form-heading').hidden = true;
      get('saved-slug').textContent = result.config.slug;
      get('saved-model').textContent = result.config.llm.modelId;
      get('saved-apps').textContent = apps.filter(app => result.config!.apps.includes(app.id)).map(app => app.name).join('、');
      get('saved-path').textContent = path;
      get('success').hidden = false; get('finish').focus();
    } catch (cause) { showError(cause instanceof Error ? cause.message : '无法连接本地服务，请检查终端是否仍在运行。'); }
    finally { setBusy(false); next.textContent = '保存配置'; }
  });

  async function load() {
    setBusy(true);
    try {
      const response = await fetch(endpoint('config'));
      if (!response.ok) throw new Error('无法读取配置，请检查终端中的提示并重新运行 apeiron init。');
      const result = await response.json() as Snapshot;
      apps = result.apps; revision = result.revision; path = result.path;
      if (result.config) applyConfig(result.config);
      else renderApps(apps.filter(app => app.selected).map(app => app.id));
      get('loading').hidden = true;
      setBusy(false); renderStep();
    } catch (cause) {
      get('loading').hidden = true;
      showError(cause instanceof Error ? cause.message : '无法连接本地配置服务，请重新运行 apeiron init。');
    }
  }
  void load();
}
