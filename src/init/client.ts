// Self-contained: embedded in both the source CLI and compiled executable.
export function initWizard(): void {
  type App = { id: string; name: string; description: string; selected: boolean; required: boolean };
  type Target = { runner: string; root: string; environment: string; profile: string; kubeconfig: string; workDir: string; image: string };
  type Config = { slug: string; apps: string[]; deployment?: Target };
  type Status = { phase: string; message: string; events: string[]; startedAt?: string; finishedAt?: string; log?: string; environment?: string; exitCode?: number };
  type Snapshot = { revision: string | null; config: Config | null; apps: App[]; defaults: Target; deployment: Status };
  const get = <T extends HTMLElement>(id: string): T => document.getElementById(id) as T;
  const form = get<HTMLFormElement>('setup-form');
  const slug = get<HTMLInputElement>('slug');
  const next = get<HTMLButtonElement>('next');
  const back = get<HTMLButtonElement>('back');
  const localTest = get<HTMLInputElement>('local-test');
  let nativeProfile = 'ubuntu';
  const fields = {
    root: get<HTMLInputElement>('chentu-root'), environment: get<HTMLInputElement>('environment'),
    kubeconfig: get<HTMLInputElement>('kubeconfig'), workDir: get<HTMLInputElement>('work-dir'), image: get<HTMLInputElement>('lab-image'),
  };
  let step = 0;
  let busy = false;
  let revision: string | null = null;
  let apps: App[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;
  const endpoint = (name: string) => new URL('api/' + name, location.href).href;
  const error = get('error');
  const titles = ['设置组织与部署环境', '选择要启用的应用'];
  const subtitles = ['填写运行 CLI 的这台机器上的路径，连接已有部署环境。', '确认应用后，直接开始 Helmfile 部署。'];
  function showError(message: string) { error.textContent = message; error.hidden = false; }
  function renderTarget() {
    const docker = localTest.checked;
    get('native-fields').hidden = docker; get('docker-fields').hidden = !docker;
    fields.kubeconfig.required = !docker;
    fields.workDir.required = docker; fields.image.required = docker;
    get('deployment-mode').textContent = docker ? '本地测试 · k3d' : 'K3s 部署';
    get('existing-profile').hidden = docker || nativeProfile !== 'local';
  }
  localTest.addEventListener('change', renderTarget);
  function target(): Target {
    return { runner: localTest.checked ? 'docker' : 'native', profile: localTest.checked ? 'local' : nativeProfile,
      root: fields.root.value.trim(), environment: fields.environment.value.trim(),
      kubeconfig: fields.kubeconfig.value.trim(), workDir: fields.workDir.value.trim(), image: fields.image.value.trim() };
  }
  function applyTarget(value: Target) {
    localTest.checked = value.runner === 'docker';
    // Existing native local-path deployments must not silently switch storage profiles.
    nativeProfile = value.runner === 'native' ? value.profile : 'ubuntu';
    get<HTMLDetailsElement>('test-options').open = localTest.checked;
    for (const key of Object.keys(fields) as Array<keyof typeof fields>) fields[key].value = value[key];
    if (!fields.image.value) fields.image.value = 'chentu-lab';
    renderTarget();
  }
  function renderStep(focus = true) {
    error.hidden = true;
    document.querySelectorAll<HTMLElement>('[data-panel]').forEach((panel, index) => { panel.hidden = index !== step; });
    document.querySelectorAll<HTMLElement>('[data-step]').forEach((item, index) => {
      item.dataset.state = index === step ? 'active' : index < step ? 'done' : 'pending';
      if (index === step) item.setAttribute('aria-current', 'step'); else item.removeAttribute('aria-current');
    });
    get('step-label').textContent = `第 ${step + 1} 步 / 共 2 步`;
    get('step-title').textContent = titles[step]!;
    get('step-subtitle').textContent = subtitles[step]!;
    back.hidden = step === 0; next.textContent = step === 1 ? '开始部署' : '下一步';
    if (focus) (step === 0 ? slug : document.querySelector<HTMLInputElement>('[name="app"]:not(:disabled)'))?.focus();
  }
  function selectedApps(): string[] {
    return Array.from(document.querySelectorAll<HTMLInputElement>('[name="app"]:checked')).map(input => input.value);
  }
  function updateSelection() {
    get('selected-count').textContent = `已选择 ${selectedApps().length} 个应用（${apps.filter(app => app.required).length} 个必选）`;
  }
  function renderApps(selected: string[]) {
    const grid = get('app-grid'); grid.replaceChildren();
    apps.forEach(app => {
      const label = document.createElement('label'); label.className = 'app-card'; label.dataset.required = String(app.required);
      const input = document.createElement('input'); input.type = 'checkbox'; input.name = 'app'; input.value = app.id;
      input.checked = app.required || selected.includes(app.id); input.disabled = app.required;
      input.addEventListener('change', updateSelection);
      const icon = document.createElement('span'); icon.className = 'app-icon'; icon.textContent = app.name.slice(0, 1); icon.setAttribute('aria-hidden', 'true');
      const copy = document.createElement('span'); copy.className = 'app-copy';
      const name = document.createElement('strong'); name.textContent = app.name;
      const description = document.createElement('span'); description.textContent = app.description;
      copy.append(name, description); label.append(input, icon, copy); grid.append(label);
      if (app.required) {
        const badge = document.createElement('span'); badge.className = 'required-badge'; badge.textContent = '必选'; label.append(badge);
      }
    }); updateSelection();
  }
  function setBusy(value: boolean) {
    busy = value; get<HTMLFieldSetElement>('fields').disabled = value; back.disabled = value; next.disabled = value;
  }
  function validate(): boolean {
    if (step === 0) {
      for (const field of [slug, fields.root, fields.environment, ...(localTest.checked ? [fields.workDir, fields.image] : [fields.kubeconfig])]) {
        if (localTest.checked && !field.validity.valid) get<HTMLDetailsElement>('test-options').open = true;
        if (!field.reportValidity()) return false;
      }
    }
    if (step === 1 && apps.some(app => app.required && !selectedApps().includes(app.id))) { showError('请保留全部必选应用。'); return false; }
    return true;
  }
  function renderDeployment(status: Status) {
    form.hidden = true; get('form-heading').hidden = true; get('deployment').hidden = false;
    document.querySelectorAll<HTMLElement>('[data-step]').forEach(item => { item.dataset.state = 'done'; item.removeAttribute('aria-current'); });
    const active = status.phase === 'preparing' || status.phase === 'running';
    get('deployment-title').textContent = active ? '正在部署…' : status.phase === 'succeeded' ? '部署完成。' : status.phase === 'cancelled' ? '部署已停止。' : '部署失败。';
    get('deployment-message').textContent = status.message;
    get('deployment-events').textContent = status.events.join('\n');
    get('deployment-log').textContent = status.log || '尚未启动 Helmfile';
    get('deployment-environment').textContent = status.environment || '尚未生成';
    get('deployment-exit').textContent = status.exitCode === undefined ? '—' : String(status.exitCode);
    get('deployment-time').textContent = status.startedAt ? `${Math.max(0, Math.floor(((status.finishedAt ? Date.parse(status.finishedAt) : Date.now()) - Date.parse(status.startedAt)) / 1000))} 秒` : '—';
    get<HTMLButtonElement>('finish').disabled = active; get('edit').hidden = active;
    get('deployment-note').textContent = active ? '关闭或刷新网页不会停止部署；在终端按 Ctrl+C 可停止部署进程。' : '完整日志保存在本机。重试会再次运行 Helmfile sync；已完成的变更不会自动回滚。';
    clearTimeout(timer);
    if (active) timer = setTimeout(() => { void poll(); }, 1000);
  }
  async function poll() {
    try {
      const response = await fetch(endpoint('deployment'));
      if (!response.ok) throw new Error();
      renderDeployment(await response.json() as Status);
    } catch {
      get('deployment-message').textContent = '暂时无法读取部署状态，正在重新连接…';
      timer = setTimeout(() => { void poll(); }, 2000);
    }
  }
  back.addEventListener('click', () => { if (!busy && step > 0) { step--; renderStep(); } });
  get('edit').addEventListener('click', () => {
    clearTimeout(timer); get('deployment').hidden = true; form.hidden = false; get('form-heading').hidden = false;
    step = 0; renderStep();
  });
  get('finish').addEventListener('click', async () => {
    const button = get<HTMLButtonElement>('finish'); button.disabled = true;
    try {
      const response = await fetch(endpoint('finish'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
      if (!response.ok) throw new Error();
      button.textContent = '向导已关闭'; get('edit').hidden = true;
      get('deployment-note').textContent = '本地部署向导已退出，你可以关闭此页面。';
    } catch { button.disabled = false; get('deployment-note').textContent = '无法关闭向导，请检查部署状态或在终端按 Ctrl+C。'; }
  });
  form.addEventListener('submit', async event => {
    event.preventDefault(); if (busy || !validate()) return;
    if (step === 0) { step++; renderStep(); return; }
    setBusy(true); next.textContent = '正在启动…'; error.hidden = true;
    try {
      const response = await fetch(endpoint('deploy'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision, slug: slug.value.trim(), apps: selectedApps(), deployment: target() }) });
      const result = await response.json() as Snapshot & { error?: string };
      if (!response.ok || !result.config) throw new Error(result.error || '启动失败，请检查配置。');
      revision = result.revision; renderDeployment(result.deployment);
    } catch (cause) { showError(cause instanceof Error ? cause.message : '无法连接本地部署服务，请检查终端。'); }
    finally { setBusy(false); next.textContent = '开始部署'; }
  });
  async function load() {
    setBusy(true);
    try {
      const response = await fetch(endpoint('config'));
      if (!response.ok) throw new Error('无法读取配置，请检查终端并重新运行 apeiron init。');
      const result = await response.json() as Snapshot;
      apps = result.apps; revision = result.revision;
      applyTarget(result.config?.deployment ?? result.defaults);
      if (result.config) slug.value = result.config.slug;
      renderApps(result.config?.apps ?? apps.filter(app => app.selected).map(app => app.id));
      get('loading').hidden = true; setBusy(false); renderStep();
      if (result.deployment.phase !== 'idle') renderDeployment(result.deployment);
    } catch (cause) { get('loading').hidden = true; showError(cause instanceof Error ? cause.message : '无法连接本地部署服务。'); }
  }
  void load();
}
