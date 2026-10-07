// Self-contained: embedded in both the source CLI and compiled executable.
export function initWizard(supportsK3sHost: typeof import('./host-platform').supportsK3sHost): void {
  type Probe = import('./probe').ProbeResult;
  type App = { id: string; name: string; description: string; selected: boolean; required: boolean };
  type Target = import('./config').DeploymentTarget;
  type Installation = import('./installation').Installation;
  type NodeFacts = import('./nodes').NodeFacts;
  type Config = { slug: string; apps: string[]; deployment?: Target };
  type Status = import('./deploy').DeploymentStatus;
  type Snapshot = { revision: string | null; config: Config | null; apps: App[]; defaults: Target; deployment: Status; host: { name: string; addresses: string[] } };
  const get = <T extends HTMLElement>(id: string): T => document.getElementById(id) as T;
  const form = get<HTMLFormElement>('setup-form');
  const slug = get<HTMLInputElement>('slug');
  const next = get<HTMLButtonElement>('next');
  const back = get<HTMLButtonElement>('back');
  const offline = get<HTMLInputElement>('offline');
  const domain = get<HTMLInputElement>('domain');
  const customDomain = get<HTMLInputElement>('custom-domain');
  const entryIp = get<HTMLInputElement>('entry-ip');
  const ha = get<HTMLInputElement>('ha');
  const hosts = get<HTMLTextAreaElement>('node-hosts');
  const fields = {
    httpPort: get<HTMLInputElement>('http-port'), httpsPort: get<HTMLInputElement>('https-port'),
    bundleDir: get<HTMLInputElement>('bundle-dir'),
    sshUser: get<HTMLInputElement>('ssh-user'), sshPort: get<HTMLInputElement>('ssh-port'), sshKey: get<HTMLInputElement>('ssh-key'),
  };
  let machine: Probe['machine'] | undefined;
  let nodeFacts: NodeFacts[] = [];
  let nodeSerial = 0;
  let deploymentAction = false;
  let deploymentRequestSerial = 0;
  let nodesFingerprint = '';
  let nodeRequest: AbortController | undefined;
  const topology = () => document.querySelector<HTMLInputElement>('[name="topology"]:checked')!.value as Installation['topology'];
  let step = 0;
  let busy = false;
  let revision: string | null = null;
  let apps: App[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;
  const endpoint = (name: string) => new URL('api/' + name, location.href).href;
  const error = get('error');
  form.addEventListener('input', () => { error.hidden = true; });
  form.addEventListener('change', () => { error.hidden = true; });
  const titles = ['设置部署环境', '设置组织', '选择要启用的应用'];
  const subtitles = ['检测当前主机，选择安装方式。', '设置团队标识与访问域名。', '准备所选应用的资源，然后创建集群并部署。'];
  let probeRequest: AbortController | undefined;
  let probeSerial = 0;
  let cliHost: Snapshot['host'] = { name: '', addresses: [] };
  let entryEdited = false;
  let entryTopology = '';
  let dnsFingerprint = '';
  let dnsSerial = 0;
  let dnsRequest: AbortController | undefined;
  let completedAccess: Status['access'];
  let currentDeployment: Status | undefined;
  let viewingDeployment = false;
  let testingPage = location.hash === '#test';
  let credentials: import('./verification').InitialAdmin | undefined;
  let credentialRequest: AbortController | undefined;
  let credentialSerial = 0;
  let verificationRequest: AbortController | undefined;
  let verificationBusy = false;
  let accessTimer: ReturnType<typeof setTimeout> | undefined;
  let accessInstalling = false;
  const accessMode = () => document.querySelector<HTMLInputElement>('[name="access-mode"]:checked')!.value;
  const publicField = (id: string) => get<HTMLInputElement>(id).value.trim();
  const defaultDomain = () => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(slug.value.trim()) ? `${slug.value.trim()}.${accessMode() === 'private' ? 'apeironlab.internal' : 'apeironlab.cn'}` : '';
  const validDomain = () => domain.value.length <= 220 && domain.value.includes('.') && domain.value.split('.').every(label => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label)) && !/^\d+(?:\.\d+){3}$/.test(domain.value);
  const validIp = () => /^(?:\d{1,3}\.){3}\d{1,3}$/.test(entryIp.value) && entryIp.value.split('.').every(part => String(Number(part)) === part && Number(part) <= 255) &&
    !/^(0|169\.254|22[4-9]|23\d|24\d|25[0-5])\./.test(entryIp.value) && (topology() === 'single-k3d' ? entryIp.value === '127.0.0.1' : !entryIp.value.startsWith('127.'));
  function renderSetup() {
    const mode = topology();
    const published = accessMode() !== 'private';
    get('public-fields').hidden = !published;
    get('gateway-fields').hidden = accessMode() !== 'relay';
    get('public-dns').hidden = !published;
    get('domain-note').textContent = published ? '使用你持有的域名。部署前将以下 DNS A 记录指向公网入口；Caddy 会自动申请并续期 HTTPS 证书。' : '内网使用无需购买域名。部署完成后，再引导你配置访问。';
    get('local-ports').hidden = mode !== 'single-k3d';
    if (entryTopology !== mode) { entryEdited = false; entryTopology = mode; }
    if (!customDomain.checked) domain.value = defaultDomain();
    domain.readOnly = !customDomain.checked;
    get('public-dns').textContent = `*.${domain.value || '<slug>.apeironlab.cn'}    A    ${publicField('public-ip') || '<公网入口 IP>'}\n${domain.value || '<slug>.apeironlab.cn'}      A    ${publicField('public-ip') || '<公网入口 IP>'}`;
    const nodes = mode === 'multi-k3s' ? target().installation!.nodes : [];
    const candidates = mode === 'single-k3d' ? ['127.0.0.1'] : mode === 'multi-k3s' ? nodes.filter(node => node.role === 'server').map(node => node.address) : cliHost.addresses;
    if (!entryEdited) entryIp.value = mode === 'multi-k3s' && ha.checked ? '' : candidates[0] ?? '';
    entryIp.readOnly = mode === 'single-k3d';
    get('entry-field').hidden = mode === 'single-k3d';
    get('entry-addresses').replaceChildren(...candidates.map(address => new Option(address, address)));
    get('entry-label').textContent = published ? '节点 IP' : '入口 IP';
    get('entry-note').textContent = published ? '部署主机网卡上的固定 IP，供 K3s 使用；用户通过上方的公网入口访问。' : mode === 'multi-k3s' && ha.checked ? '填写已配置的稳定入口 IP（负载均衡或 VIP）；向导不会自动创建 VIP。' : mode === 'multi-k3s' ? '默认使用控制节点 IP，也可填写已配置的入口 IP；固定到单个节点不提供入口高可用。' : '从 CLI 主机网卡建议，请确认这是访问设备可达的固定 IP。';
  }
  function renderAccessGuide(access: NonNullable<Status['access']>) {
    completedAccess = access;
    get('completed-dns-guide').textContent = access.local ? `将下载的完整 hosts 配置合并到运行 CLI 的这台机器。本机 K3d HTTPS 入口为 127.0.0.1:${access.httpsPort ?? 443}。下面是 Apeiron 和 IAM 的示例。` : '在内网 DNS 添加以下 A 记录，并让访问设备使用该 DNS。少量工作站也可使用下载的 hosts 配置。';
    get('dns-records').textContent = access.local
      ? `127.0.0.1 apeiron.${access.domain}\n127.0.0.1 iam.${access.domain}`
      : `*.${access.domain}    A    ${access.entryIp}\n${access.domain}      A    ${access.entryIp}`;
    get('dns-scope').textContent = `检测发起于 CLI 主机 ${cliHost.name || '当前主机'}。${access.local ? '检查 Apeiron 与 IAM 的本机解析。' : '检查 Apeiron、IAM 和随机子域名的泛解析。'}浏览器若在其他电脑上，需在那台电脑上配置解析与证书信任；此检查不代表 HTTPS 或应用已经可用。`;
    const fingerprint = JSON.stringify([access.domain, access.entryIp, access.local]);
    if (dnsFingerprint !== fingerprint) {
      dnsFingerprint = fingerprint; resetDns();
    }
  }
  function resetDns() {
    dnsSerial++; dnsRequest?.abort();
    get('dns-results').replaceChildren(); get('dns-status').textContent = '完成解析配置后，点击检测。'; get('dns-status').dataset.state = '';
    get('copy-dns-status').textContent = '';
    get<HTMLButtonElement>('check-dns').disabled = false;
  }
  for (const input of [slug, domain]) input.addEventListener('input', renderSetup);
  customDomain.addEventListener('change', renderSetup);
  for (const input of document.querySelectorAll<HTMLInputElement>('[name="access-mode"]')) input.addEventListener('change', renderSetup);
  get('public-ip').addEventListener('input', renderSetup);
  entryIp.addEventListener('input', () => { entryEdited = true; renderSetup(); });
  get('node-results').addEventListener('change', renderSetup);
  get('copy-dns').addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(get('dns-records').textContent ?? ''); get('copy-dns-status').textContent = '配置已复制'; }
    catch { get('copy-dns-status').textContent = '请选中上面的配置并手动复制。'; }
  });
  get('check-dns').addEventListener('click', async () => {
    const access = completedAccess;
    if (!access) return;
    const serial = ++dnsSerial; dnsRequest?.abort(); dnsRequest = new AbortController();
    const button = get<HTMLButtonElement>('check-dns'); button.disabled = true;
    get('dns-status').textContent = '正在检测解析…'; get('dns-status').dataset.state = '';
    try {
      const response = await fetch(endpoint('dns'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ domain: access.domain, entryIp: access.entryIp, local: access.local }), signal: dnsRequest.signal });
      const result = await response.json() as import('./dns').DnsResult & { error?: string };
      if (serial !== dnsSerial) return;
      if (!response.ok) throw new Error(result.error || '无法完成解析检查。');
      get('dns-status').textContent = result.passed ? `CLI 主机 ${result.checkedFrom} 解析通过` : '解析未通过，请按配置说明检查 DNS / hosts。';
      get('dns-status').dataset.state = result.passed ? 'passed' : 'failed';
      get('dns-results').replaceChildren(...result.checks.map(check => {
        const row = document.createElement('li');
        row.textContent = `${check.host} → ${check.addresses.join(', ') || (check.status === 'timeout' ? '检测超时' : '未解析')} · ${check.status === 'matched' ? '匹配' : '未通过'}`;
        return row;
      }));
      if (!result.passed) { get<HTMLDetailsElement>('manual-access').open = true; get<HTMLDetailsElement>('dns-guide').open = true; }
    } catch (cause) {
      if (serial === dnsSerial) { get('dns-status').textContent = cause instanceof Error ? cause.message : '解析检查失败。'; get('dns-status').dataset.state = 'failed'; }
    } finally { if (serial === dnsSerial) button.disabled = false; }
  });
  function renderNetworkCards(checks: Probe['network']['checks'], fallback = '检测中…') {
    const results = new Map(checks.map(check => [check.host, check]));
    const labels = { reachable: '已连接', 'http-error': '响应异常', 'dns-error': '解析失败', 'tls-error': '连接异常', timeout: '连接超时', unreachable: '无法连接', cancelled: '已取消' };
    for (const card of get('probe-cards').querySelectorAll<HTMLElement>('[data-host]')) {
      const check = results.get(card.dataset.host!);
      card.dataset.state = check ? check.status === 'reachable' && check.httpStatus !== 404 ? 'reachable' : 'limited' : fallback === '检测中…' ? 'pending' : fallback === '检测失败' ? 'limited' : 'skipped';
      card.querySelector('.network-status')!.textContent = check ? check.status === 'reachable' && check.host === 'release-assets.githubusercontent.com' ? '域名可达' : labels[check.status] : fallback;
      card.querySelector('.network-latency')!.textContent = check ? `${check.elapsedMs} ms` : '—';
    }
  }
  async function probeEnvironment(refresh = false) {
    const serial = ++probeSerial;
    probeRequest?.abort();
    probeRequest = new AbortController();
    const button = get<HTMLButtonElement>('probe-refresh');
    button.disabled = true; button.textContent = '正在检测…';
    get('machine-probe').setAttribute('aria-busy', 'true');
    get('system-probe').setAttribute('aria-busy', 'true');
    const network = get('probe-network');
    network.textContent = offline.checked ? '离线模式 · 已跳过' : '检测中…';
    network.dataset.state = 'pending';
    renderNetworkCards([], offline.checked ? '已跳过' : '检测中…');
    get('probe-sites').replaceChildren();
    get('probe-note').textContent = offline.checked ? '离线模式下不发起外网探测。' : '正在检查 Microsoft、Google、GitHub API 和安装包下载域名，单项最多等待 4 秒。';
    try {
      const response = await fetch(endpoint('probe'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ offline: offline.checked, refresh }), signal: probeRequest.signal });
      if (!response.ok) throw new Error();
      const result = await response.json() as Probe;
      if (serial !== probeSerial) return;
      machine = result.machine; renderTarget();
      get('probe-os').textContent = [result.machine.os.name, result.machine.os.version].filter(Boolean).join(' ');
      get('probe-kernel').textContent = '内核 ' + result.machine.os.kernel;
      const hardware = result.machine.hardware;
      get('probe-arch').textContent = hardware.architecture.toUpperCase();
      get('probe-cores').textContent = `${hardware.cores} 核`;
      get('probe-memory').textContent = `${hardware.memoryGiB} GiB`;
      get('probe-cpu').textContent = (hardware.cpu || 'CPU 型号未提供') + (hardware.architecture !== hardware.runtimeArchitecture ? ` · CLI 运行架构 ${hardware.runtimeArchitecture}` : '');
      const labels = { reachable: '探测点均可连接', limited: '部分探测点可连接', unreachable: '探测点均未通过', skipped: '离线模式 · 已跳过', cancelled: '探测已取消' };
      network.textContent = labels[result.network.status]; network.dataset.state = result.network.status;
      renderNetworkCards(result.network.checks, result.network.status === 'skipped' ? '已跳过' : '已取消');
      const statuses = { reachable: '已连接', 'http-error': '已响应，HTTP 错误', 'dns-error': 'DNS 解析失败', 'tls-error': 'TLS 连接失败', timeout: '连接超时', unreachable: '连接失败', cancelled: '已取消' };
      for (const check of result.network.checks) {
        const row = document.createElement('li');
        const site = document.createElement('span'); site.textContent = `${check.name} · ${check.host}`;
        const state = document.createElement('span');
        state.dataset.state = check.status === 'reachable' && check.httpStatus !== 404 ? 'reachable' : 'limited';
        state.textContent = `${statuses[check.status]}${check.httpStatus === undefined ? '' : ` · HTTP ${check.httpStatus}`} · ${check.elapsedMs} ms`;
        if (check.host === 'release-assets.githubusercontent.com') {
          const detail = document.createElement('small');
          detail.textContent = check.httpStatus === 404 ? '已收到服务器响应，但根路径没有资源。尚未验证具体安装包能否下载。' : '这里只检测下载域名；具体安装包能否下载，需要在获取安装包时验证。';
          row.append(detail);
        }
        row.prepend(site, state); get('probe-sites').append(row);
      }
      get('probe-note').textContent = result.network.status === 'skipped' ? '离线模式下不发起外网探测。' : '结果仅代表上述站点；Google 可连接不代表所有海外网站可用。部署模式由你选择。';
      get('probe-time').textContent = '检测于 ' + new Date(result.checkedAt).toLocaleTimeString();
    } catch {
      if (serial !== probeSerial) return;
      network.textContent = '检测未完成'; network.dataset.state = 'unreachable';
      renderNetworkCards([], '检测失败');
      get('probe-note').textContent = '无法读取本机探测结果，请重新检测或检查 CLI 是否仍在运行。';
    } finally {
      if (serial === probeSerial) {
        get('machine-probe').setAttribute('aria-busy', 'false');
        get('system-probe').setAttribute('aria-busy', 'false');
        button.disabled = false; button.textContent = '重新检测';
      }
    }
  }
  get('probe-refresh').addEventListener('click', () => { void probeEnvironment(true); });
  function showError(message: string) { error.textContent = message; error.hidden = false; }
  function renderTarget() {
    const mode = topology();
    get('node-section').hidden = mode !== 'multi-k3s';
    get('ha-fields').hidden = mode !== 'multi-k3s';
    get('deployment-mode').textContent = mode === 'single-k3d' ? '本地测试 · K3d' : mode === 'multi-k3s' ? '多机 K3s' : '单机 K3s';
    fields.bundleDir.required = offline.checked;
    get('bundle-fields').hidden = !offline.checked;
    get('resource-mode').textContent = offline.checked ? '检查本地应用及依赖包；缺少或校验失败时停止，不访问外网。' : '选择应用后检查实际安装资源，下载与校验通过后才会安装。';
    const supported = machine && supportsK3sHost(machine.os.name, machine.os.version, machine.hardware.architecture);
    get('topology-note').textContent = mode === 'single-k3d' ? offline.checked ? '使用本地完整安装包创建 K3d 测试集群，需要本机 Docker 正在运行。' : '创建独立的本地测试集群，需要本机 Docker 正在运行。' : mode === 'multi-k3s' ? '管理机可以使用 macOS；节点需为 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64，系统与架构一致，并提供匹配的安装包。' : machine && !supported ? '单机 K3s 需要 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64。' : '在当前 Ubuntu 主机上安装 K3s；需要匹配系统与架构的安装包，以及 root 或免密 sudo。';
    renderSetup();
  }
  for (const input of document.querySelectorAll<HTMLInputElement>('[name="topology"]')) input.addEventListener('change', renderTarget);
  for (const input of document.querySelectorAll<HTMLInputElement>('[name="source"]')) input.addEventListener('change', () => { renderTarget(); void probeEnvironment(true); });
  function connection() { return { sshUser: fields.sshUser.value.trim(), sshKey: fields.sshKey.value.trim(), sshPort: Number(fields.sshPort.value) }; }
  function nodeHosts() { return hosts.value.trim().split(/[\s,]+/).filter(Boolean); }
  function fingerprint() { return JSON.stringify({ hosts: nodeHosts(), ...connection() }); }
  function invalidateNodes() {
    nodeSerial++; nodeRequest?.abort(); nodesFingerprint = ''; nodeFacts = [];
    get('node-results').replaceChildren(); get('nodes-note').textContent = '连接设置已更改，请重新检测节点。';
    renderSetup();
    const button = get<HTMLButtonElement>('detect-nodes'); button.disabled = false; button.textContent = '自动检测节点';
  }
  for (const input of [hosts, fields.sshUser, fields.sshKey, fields.sshPort]) input.addEventListener('input', invalidateNodes);
  function renderNodes(saved?: Installation['nodes']) {
    get('node-results').replaceChildren();
    nodeFacts.forEach((facts, index) => {
      const row = document.createElement('article'); row.className = 'node-result'; row.dataset.host = facts.host;
      const header = document.createElement('header'); const title = document.createElement('strong'); title.textContent = facts.host;
      const status = document.createElement('span'); status.textContent = facts.error ? '需要处理' : '检测通过'; header.append(title, status); row.append(header);
      const detail = document.createElement('p'); detail.textContent = facts.os ? `${facts.os} ${facts.version} · ${facts.architecture} · ${facts.cores} 核 · ${facts.memoryGiB} GiB 内存 · ${facts.diskGiB} GiB 可用磁盘` : '未能连接'; row.append(detail);
      if (facts.error) { const note = document.createElement('p'); note.className = 'node-error'; note.textContent = facts.error; row.append(note); }
      if (facts.name) {
        const controls = document.createElement('div'); controls.className = 'node-controls';
        const previous = saved?.find(node => node.host === facts.host);
        const nameLabel = document.createElement('label'); nameLabel.textContent = '节点名称';
        const name = document.createElement('input'); name.type = 'text'; name.dataset.field = 'name'; name.value = previous?.name ?? facts.name; nameLabel.append(name);
        const addressLabel = document.createElement('label'); addressLabel.textContent = '节点内网 IP';
        const address = document.createElement('select'); address.dataset.field = 'address';
        for (const ip of facts.addresses) address.add(new Option(ip, ip));
        if (previous && facts.addresses.includes(previous.address)) address.value = previous.address;
        else if (facts.addresses.includes(facts.host)) address.value = facts.host;
        addressLabel.append(address);
        const roleLabel = document.createElement('label'); roleLabel.textContent = '节点角色';
        const role = document.createElement('select'); role.dataset.field = 'role'; role.add(new Option('控制节点', 'server')); role.add(new Option('工作节点', 'agent'));
        role.value = previous?.role ?? (index < (ha.checked ? 3 : 1) ? 'server' : 'agent'); roleLabel.append(role);
        controls.append(nameLabel, addressLabel, roleLabel); row.append(controls);
      }
      get('node-results').append(row);
    });
    renderSetup();
  }
  ha.addEventListener('change', () => renderNodes());
  get('detect-nodes').addEventListener('click', async () => {
    const serial = ++nodeSerial; nodeRequest?.abort(); nodeRequest = new AbortController();
    const requested = fingerprint(); nodesFingerprint = '';
    const button = get<HTMLButtonElement>('detect-nodes'); button.disabled = true; button.textContent = '正在检测…';
    get('nodes-note').textContent = '通过 SSH 检测节点，每次最多并行检测 4 台。';
    try {
      const response = await fetch(endpoint('nodes'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: requested, signal: nodeRequest.signal });
      const result = await response.json() as { nodes?: NodeFacts[]; error?: string };
      if (serial !== nodeSerial) return;
      if (!response.ok || !result.nodes) throw new Error(result.error || '节点检测失败。');
      nodeFacts = result.nodes; nodesFingerprint = requested; renderNodes();
      error.hidden = true;
      get('nodes-note').textContent = nodeFacts.some(node => node.error) ? '请处理节点提示后重新检测。' : '已生成节点配置，请确认内网 IP 和控制／工作角色。部署前会再次核验。';
    } catch (cause) { if (serial === nodeSerial) get('nodes-note').textContent = cause instanceof Error ? cause.message : '节点检测失败。'; }
    finally { if (serial === nodeSerial) { button.disabled = false; button.textContent = '自动检测节点'; } }
  });
  get('import-ssh').addEventListener('click', async () => {
    try {
      const response = await fetch(endpoint('ssh-aliases'));
      const result = await response.json() as { aliases?: string[] };
      if (!response.ok || !result.aliases?.length) throw new Error('没有可导入的 SSH 别名，请手动填写地址。');
      hosts.value = result.aliases.join('\n'); invalidateNodes();
      get('nodes-note').textContent = '已导入 SSH 别名。请保留要部署的机器，再点击自动检测。';
    } catch (cause) { get('nodes-note').textContent = cause instanceof Error ? cause.message : '无法读取 SSH 配置。'; }
  });
  get('pick-bundle').addEventListener('click', async () => {
    const button = get<HTMLButtonElement>('pick-bundle'); button.disabled = true;
    try {
      const response = await fetch(endpoint('pick-bundle'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
      const result = await response.json() as { path?: string; error?: string };
      if (!response.ok) throw new Error(result.error || '请手动填写目录路径。');
      if (result.path) fields.bundleDir.value = result.path;
    } catch (cause) { showError(cause instanceof Error ? cause.message : '请手动填写目录路径。'); }
    finally { button.disabled = false; }
  });
  function target(): Target {
    const mode = topology();
    const nodes = mode === 'multi-k3s' ? Array.from(get('node-results').querySelectorAll<HTMLElement>('.node-result')).map(row => ({
      host: row.dataset.host!, name: row.querySelector<HTMLInputElement>('[data-field=name]')?.value.trim() ?? '',
      address: row.querySelector<HTMLSelectElement>('[data-field=address]')?.value ?? '',
      role: row.querySelector<HTMLSelectElement>('[data-field=role]')?.value as 'server' | 'agent',
    })) : [];
    return { installation: { topology: mode, domain: domain.value.trim(), entryIp: entryIp.value.trim(), httpPort: mode === 'single-k3d' ? Number(fields.httpPort.value) : 80, httpsPort: mode === 'single-k3d' ? Number(fields.httpsPort.value) : 443, ha: mode === 'multi-k3s' && ha.checked, ...connection(), nodes, ...(accessMode() !== 'private' ? { publicAccess: { mode: accessMode() as 'direct' | 'relay', publicIp: publicField('public-ip'), tunnelPort: Number(publicField('tunnel-port')), ...(accessMode() === 'relay' ? { gateway: { host: publicField('gateway-host'), sshUser: publicField('gateway-user'), sshKey: publicField('gateway-key'), sshPort: Number(publicField('gateway-port')) } } : {}) } } : {}) },
      runner: mode === 'single-k3d' ? 'docker' : 'native',
      root: '', offline: offline.checked, bundleDir: offline.checked ? fields.bundleDir.value.trim() : '',
      environment: '', kubeconfig: '', workDir: '', image: 'chentu-lab' };
  }
  function applyTarget(value: Target) {
    const install = value.installation;
    const published = install?.publicAccess;
    document.querySelector<HTMLInputElement>(`[name="access-mode"][value="${published?.mode ?? 'private'}"]`)!.checked = true;
    for (const [id, text] of Object.entries({ 'public-ip': published?.publicIp ?? '', 'gateway-host': published?.gateway?.host ?? '', 'gateway-user': published?.gateway?.sshUser ?? '', 'gateway-key': published?.gateway?.sshKey ?? '', 'gateway-port': String(published?.gateway?.sshPort ?? 22), 'tunnel-port': String(published?.tunnelPort ?? 19444) })) get<HTMLInputElement>(id).value = text;
    document.querySelector<HTMLInputElement>(`[name="topology"][value="${install?.topology ?? 'single-k3d'}"]`)!.checked = true;
    offline.checked = Boolean(value.offline); get<HTMLInputElement>('online').checked = !offline.checked;
    fields.bundleDir.value = value.bundleDir; domain.value = install?.domain ?? '';
    customDomain.checked = Boolean(domain.value && domain.value !== defaultDomain());
    get<HTMLDetailsElement>('custom-domain').closest('details')!.open = customDomain.checked;
    entryIp.value = install?.entryIp ?? ''; entryEdited = Boolean(entryIp.value); entryTopology = topology();
    fields.httpPort.value = String(install?.httpPort ?? 54320); fields.httpsPort.value = String(install?.httpsPort ?? 54321);
    fields.sshUser.value = install?.sshUser ?? ''; fields.sshKey.value = install?.sshKey ?? ''; fields.sshPort.value = String(install?.sshPort ?? 22);
    ha.checked = install?.ha ?? false; hosts.value = install?.nodes.map(node => node.host).join('\n') ?? '';
    renderTarget();
  }
  function renderStep(focus = true) {
    renderSetup();
    error.hidden = true;
    document.querySelectorAll<HTMLElement>('[data-panel]').forEach((panel, index) => { panel.hidden = index !== step; });
    renderProgress(step);
    get('step-label').textContent = `第 ${step + 1} 步 / 共 6 步`;
    get('step-title').textContent = titles[step]!;
    get('step-subtitle').textContent = subtitles[step]!;
    back.hidden = step === 0; next.textContent = step === titles.length - 1 ? '开始部署' : '下一步';
    if (focus) (step === 0 ? get<HTMLInputElement>('online') : step === 1 ? slug : document.querySelector<HTMLInputElement>('[name="app"]:not(:disabled)'))?.focus();
  }
  function renderProgress(active: number) {
    document.querySelectorAll<HTMLElement>('[data-step]').forEach((item, index) => {
      item.dataset.state = index === active ? 'active' : index < active ? 'done' : 'pending';
      if (index === active) item.setAttribute('aria-current', 'step'); else item.removeAttribute('aria-current');
    });
  }
  function selectedApps(): string[] {
    return Array.from(document.querySelectorAll<HTMLInputElement>('[name="app"]:checked')).map(input => input.value);
  }
  function updateSelection() {
    get('selected-count').textContent = `已选择 ${selectedApps().length} 个应用（${apps.filter(app => app.required).length} 个必选）`;
  }
  function renderApps(selected: string[]) {
    const grid = get('app-grid'); grid.replaceChildren();
    let previousGroup = '';
    apps.forEach(app => {
      const group = app.required ? '必选应用' : app.selected ? '可选应用 · 默认选中' : '其他可选应用 · 默认不选';
      if (group !== previousGroup) {
        const heading = document.createElement('h2'); heading.className = 'app-group'; heading.textContent = group;
        grid.append(heading); previousGroup = group;
      }
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
      if (topology() === 'single-k3d' && (![fields.httpPort.value, fields.httpsPort.value].every(value => /^\d{1,5}$/.test(value) && Number(value) >= 1 && Number(value) <= 65535) || Number(fields.httpPort.value) === Number(fields.httpsPort.value) || Number(fields.httpPort.value) === 443 || Number(fields.httpsPort.value) === 80)) { showError('请填写不同的 HTTP / HTTPS 端口（1–65535）；HTTP 不能使用 443，HTTPS 不能使用 80。'); return false; }
      if (offline.checked && !fields.bundleDir.reportValidity()) return false;
      if (topology() === 'single-k3s' && (!machine || !supportsK3sHost(machine.os.name, machine.os.version, machine.hardware.architecture))) { showError(machine ? '单机 K3s 需要 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64。' : '请先完成当前主机的系统检测。'); return false; }
      if (topology() === 'multi-k3s') {
        if (nodesFingerprint !== fingerprint() || nodeFacts.length < 2 || nodeFacts.some(node => node.error)) { showError('请先成功检测至少 2 台 Ubuntu 节点。'); return false; }
        const nodes = target().installation!.nodes;
        const count = nodes.filter(node => node.role === 'server').length;
        if (ha.checked ? count < 3 || count % 2 === 0 : count !== 1) { showError(ha.checked ? '高可用需要至少 3 个、且为奇数个控制节点。' : '请选择 1 个控制节点，其余为工作节点。'); return false; }
        for (const key of ['host', 'name', 'address'] as const) if (new Set(nodes.map(node => node[key])).size !== nodes.length) { showError('节点名称和地址不能重复。'); return false; }
      }
    }
    if (step === 0 && accessMode() !== 'private' && (topology() !== 'single-k3s' || offline.checked)) { showError('公网入口目前支持在线单机 K3s。'); return false; }
    if (step === 0 && accessMode() !== 'private' && (!publicField('public-ip') || (accessMode() === 'relay' && !publicField('gateway-host')))) { showError('请填写公网入口 IP 和 ECS SSH 连接。'); return false; }
    if (step === 0 && !validIp()) { showError('请确认部署环境中的入口 IPv4 地址。'); return false; }
    if (step === 1) {
      if (!slug.reportValidity() || !domain.reportValidity()) return false;
      if (!validDomain()) { showError('请填写有效的平台域名。'); return false; }
    }
    if (step === 2 && apps.some(app => app.required && !selectedApps().includes(app.id))) { showError('请保留全部必选应用。'); return false; }
    return true;
  }
  function renderDeployment(status: Status) {
    currentDeployment = status;
    form.hidden = true; get('form-heading').hidden = true; get('finish-actions').hidden = false;
    const access = status.phase === 'succeeded' ? status.access : undefined;
    const showTest = Boolean(access && !viewingDeployment && testingPage);
    const showAccess = Boolean(access && !viewingDeployment && !testingPage);
    get('deployment').hidden = showAccess || showTest;
    get('verification').hidden = !showTest;
    renderProgress(showTest ? 5 : showAccess ? 4 : 3);
    const active = ['preparing', 'running', 'stopping'].includes(status.phase);
    get('deployment-title').textContent = status.phase === 'stopping' ? (status.stopFailed ? '停止尚未完成。' : '正在停止部署…') : active ? '正在部署…' : status.phase === 'succeeded' ? '部署完成。' : status.phase === 'cancelled' ? '部署已停止。' : '部署失败。';
    get('deployment-message').textContent = status.message;
    get('deployment-events').replaceChildren(...status.events.map(message => {
      const line = document.createElement('span');
      const level = /^\[(INFO|WARNING|ERROR)\]/.exec(message)?.[1] || 'INFO';
      line.className = `log-${level.toLowerCase()}`;
      line.textContent = message + '\n';
      return line;
    }));
    get('deployment-log').textContent = status.log || '正在检查安装资源';
    const openLog = get<HTMLAnchorElement>('open-log');
    const downloadLog = get<HTMLAnchorElement>('download-log');
    openLog.hidden = downloadLog.hidden = !status.log;
    openLog.href = endpoint('log'); downloadLog.href = endpoint('log/download');
    get('deployment-environment').textContent = status.environment || '尚未生成';
    get('deployment-exit').textContent = status.exitCode === undefined ? '—' : String(status.exitCode);
    get('deployment-time').textContent = status.startedAt ? `${Math.max(0, Math.floor(((status.finishedAt ? Date.parse(status.finishedAt) : Date.now()) - Date.parse(status.startedAt)) / 1000))} 秒` : '—';
    get<HTMLButtonElement>('finish').disabled = active || accessInstalling || verificationBusy; get('edit').hidden = active || showAccess || showTest;
    get('finish').hidden = Boolean(access && !showTest);
    get('view-deployment').hidden = !showAccess && !showTest;
    get('configure-access').hidden = !access || showAccess || showTest;
    get('to-verification').hidden = !showAccess;
    get('back-access').hidden = !showTest;
    const stopButton = get<HTMLButtonElement>('stop-deployment');
    stopButton.hidden = !active;
    stopButton.disabled = deploymentAction || (status.phase === 'stopping' && !status.stopFailed);
    stopButton.textContent = status.stopFailed ? '重试停止' : status.phase === 'stopping' ? '正在停止…' : '停止部署';
    const retryButton = get<HTMLButtonElement>('retry-deployment');
    retryButton.hidden = !['failed', 'cancelled'].includes(status.phase);
    retryButton.disabled = deploymentAction;
    get<HTMLButtonElement>('edit').disabled = deploymentAction;
    if (deploymentAction) get<HTMLButtonElement>('finish').disabled = true;
    get('deployment-note').textContent = active ? '关闭或刷新页面不会停止部署。点击“停止部署”会终止部署进程，保留已有资源；已提交给集群的任务可能继续运行。' : status.phase === 'cancelled'
      ? '点击“重新部署”使用上次保存的配置，检查资源和 Helm 状态后重新运行。不会断点续跑，也不会删除已有集群和数据。'
      : status.exitCode === undefined
      ? '安装在准备阶段停止。请根据日志修正原因后重试；已完成的变更不会自动回滚。'
      : '日志可在网页查看或下载。重试会重新检查资源并执行部署；已完成的变更不会自动回滚。';
    get('access-complete').hidden = !showAccess;
    if (access) {
      renderAccessGuide(access);
      for (const id of ['private-access-card', 'private-access-note', 'manual-access']) get(id).hidden = Boolean(access.public);
      get('access-complete').querySelector('.subtitle')!.textContent = access.public ? '公网 HTTPS 入口已验证。无需配置 hosts 或安装 CA，可以进入测试。' : '部署已完成。配置这台电脑的访问方式，然后进入测试。';
      get('access-entry').textContent = `https://apeiron.${access.domain}${(access.httpsPort ?? 443) === 443 ? '' : ':' + access.httpsPort} · ${access.public ? '公网访问' : access.local ? '本机测试' : '内网访问'} · ${access.entryIp}:${access.httpsPort ?? 443}`;
      get('access-notes').textContent = access.notes.join(' ');
      const ca = get<HTMLAnchorElement>('download-ca'); ca.hidden = !access.ca; ca.href = endpoint('ca.crt');
      const hosts = get<HTMLAnchorElement>('download-hosts'); hosts.hidden = !access.hostsPath; hosts.href = endpoint('hosts.txt');
      get('ca-path').textContent = access.ca?.path ?? '未生成';
      get('ca-fingerprint').textContent = access.ca?.fingerprint ?? '—';
      get('ca-expiry').textContent = access.ca ? new Date(access.ca.expiresAt).toLocaleDateString() : '—';
    }
    if (!access) { completedAccess = undefined; dnsFingerprint = ''; resetDns(); }
    clearTimeout(timer);
    if (active) timer = setTimeout(() => { void poll(); }, 1000);
    if (showAccess && !access?.public) void loadLocalAccess();
    if (showTest && access) {
      get<HTMLAnchorElement>('test-open-apeiron').href = `https://apeiron.${access.domain}${(access.httpsPort ?? 443) === 443 ? '' : ':' + access.httpsPort}/`;
      get<HTMLAnchorElement>('test-open-ops').href = `https://ops.${access.domain}${(access.httpsPort ?? 443) === 443 ? '' : ':' + access.httpsPort}/`;
      get('verification-scope').textContent = `检测发起于 CLI 主机 ${cliHost.name}，使用系统 DNS 与证书信任，直连部署入口。浏览器在其他电脑上时，需要在那台电脑上另行确认。`;
      if (!credentials) void loadCredentials();
      void fetch(endpoint('verification')).then(response => { if (!response.ok) throw new Error(); return response.json(); })
        .then(result => { if (result.result && testingPage) renderVerification(result.result); }).catch(() => {});
    }
  }
  function renderLocalAccess(result: { capability: import('./local-access').LocalAccessCapability; status: import('./local-access').LocalAccessStatus }) {
    const { capability, status } = result;
    accessInstalling = status.phase === 'installing';
    const button = get<HTMLButtonElement>('install-access');
    button.hidden = !capability.available;
    button.disabled = accessInstalling || !completedAccess?.ca || !completedAccess?.hostsPath;
    button.textContent = accessInstalling ? '正在配置…' : status.phase === 'succeeded' ? '重新配置本机访问' : status.phase === 'failed' || status.phase === 'cancelled' ? '重试配置本机访问' : '一键配置本机访问';
    get('local-access-host').textContent = `将配置 CLI 主机：${capability.host}`;
    get('local-access-description').hidden = !capability.available;
    get('local-access-status').textContent = !capability.available ? capability.reason : status.message || (!completedAccess?.ca || !completedAccess?.hostsPath ? '本次部署缺少访问文件，请查看下方说明。' : '系统会保留 hosts 原有条目，并在修改前备份。');
    get('local-access-status').dataset.state = status.phase;
    get('local-access-backup').textContent = status.backup ? `hosts 备份：${status.backup}` : '';
    get('local-access-checks').replaceChildren(...(status.checks ?? []).map(check => {
      const li = document.createElement('li'); li.textContent = `${check.passed ? '✓' : '○'} ${check.host} · ${check.passed ? '解析与 HTTPS 通过' : '访问检查未通过'}`; return li;
    }));
    for (const id of ['finish', 'view-deployment', 'edit', 'to-verification']) get<HTMLButtonElement>(id).disabled = accessInstalling;
    clearTimeout(accessTimer);
    if (accessInstalling) accessTimer = setTimeout(() => { void loadLocalAccess(); }, 1000);
  }
  async function loadLocalAccess() {
    try {
      const response = await fetch(endpoint('access'));
      if (!response.ok) throw new Error();
      renderLocalAccess(await response.json());
    } catch {
      get('local-access-status').textContent = '无法读取本机配置状态，请刷新页面重试。';
      if (accessInstalling) accessTimer = setTimeout(() => { void loadLocalAccess(); }, 2000);
    }
  }
  get('install-access').addEventListener('click', async () => {
    if (!completedAccess || accessInstalling) return;
    accessInstalling = true; get<HTMLButtonElement>('install-access').disabled = true;
    for (const id of ['finish', 'view-deployment', 'edit', 'to-verification']) get<HTMLButtonElement>(id).disabled = true;
    get('local-access-status').textContent = '正在请求 macOS 管理员授权…';
    try {
      const response = await fetch(endpoint('access/install'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || '无法开始本机配置。');
      await loadLocalAccess();
    } catch (cause) {
      accessInstalling = false;
      for (const id of ['install-access', 'finish', 'view-deployment', 'edit', 'to-verification']) get<HTMLButtonElement>(id).disabled = false;
      get('local-access-status').textContent = cause instanceof Error ? cause.message : '无法开始本机配置，请重试。';
    }
  });
  function clearCredentials() {
    credentialSerial++; credentialRequest?.abort(); credentials = undefined;
    get<HTMLInputElement>('admin-username').value = ''; get<HTMLInputElement>('admin-password').value = '';
    get<HTMLInputElement>('admin-password').type = 'password'; get('toggle-password').textContent = '显示密码';
    get('toggle-password').setAttribute('aria-pressed', 'false');
    for (const id of ['copy-username', 'copy-password', 'toggle-password', 'download-credentials']) get<HTMLButtonElement>(id).disabled = true;
  }
  async function loadCredentials() {
    clearCredentials(); const serial = credentialSerial;
    credentialRequest = new AbortController();
    get('credentials-status').textContent = '正在读取本次部署的初始管理员凭据…';
    get<HTMLButtonElement>('reload-credentials').disabled = true;
    try {
      const response = await fetch(endpoint('credentials'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}', signal: credentialRequest.signal });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || '无法读取凭据，请重试。');
      if (serial !== credentialSerial || !testingPage) return;
      credentials = result;
      get<HTMLInputElement>('admin-username').value = result.username;
      get<HTMLInputElement>('admin-password').value = result.password;
      get('credentials-status').textContent = '凭据已就绪。密码默认隐藏，可显示或复制。';
      for (const id of ['copy-username', 'copy-password', 'toggle-password', 'download-credentials']) get<HTMLButtonElement>(id).disabled = false;
    } catch (cause) {
      if (serial === credentialSerial) get('credentials-status').textContent = cause instanceof Error ? cause.message : '无法读取凭据，请重试。';
    } finally { if (serial === credentialSerial) get<HTMLButtonElement>('reload-credentials').disabled = false; }
  }
  get('reload-credentials').addEventListener('click', () => { void loadCredentials(); });
  get('toggle-password').addEventListener('click', () => {
    const input = get<HTMLInputElement>('admin-password'); const show = input.type === 'password';
    input.type = show ? 'text' : 'password'; get('toggle-password').textContent = show ? '隐藏密码' : '显示密码';
    get('toggle-password').setAttribute('aria-pressed', String(show));
  });
  for (const key of ['username', 'password'] as const) get('copy-' + key).addEventListener('click', async () => {
    if (!credentials) return;
    try { await navigator.clipboard.writeText(credentials[key]); get('credentials-status').textContent = `${key === 'username' ? '用户名' : '密码'}已复制。`; }
    catch { get('credentials-status').textContent = '无法访问剪贴板，请显示后选中对应字段复制。'; }
  });
  get('download-credentials').addEventListener('click', () => {
    if (!credentials) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify(credentials, null, 2) + '\n'], { type: 'application/json' }));
    const link = document.createElement('a'); link.href = url; link.download = 'apeiron-admin.json'; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  function renderVerification(result: import('./verification').VerificationResult) {
    get('verification-results').replaceChildren(...result.checks.map(check => {
      const li = document.createElement('li'); const name = document.createElement('strong'); name.textContent = `${check.name} · ${check.host}`;
      const message = document.createElement('span'); message.textContent = check.message; message.dataset.state = check.https === 'passed' ? 'passed' : 'failed';
      const details = document.createElement('small'); details.textContent = `解析${check.dns === 'passed' ? '通过' : '未通过'} · HTTPS ${check.https === 'passed' ? '通过' : check.https === 'skipped' ? '未检测' : '未通过'}`;
      li.append(name, message, details); return li;
    }));
    get('verification-status').textContent = `${result.passed ? '连接测试通过，请继续下方登录测试。' : '部分检查未通过，请返回配置访问或检查应用状态。'} 检测于 ${new Date(result.checkedAt).toLocaleTimeString()}`;
    get('verification-status').dataset.state = result.passed ? 'passed' : 'failed';
  }
  get('run-verification').addEventListener('click', async () => {
    if (verificationBusy) return;
    verificationBusy = true; verificationRequest = new AbortController();
    for (const id of ['run-verification', 'finish']) get<HTMLButtonElement>(id).disabled = true;
    get('verification-status').textContent = '正在检查解析、证书信任和 HTTPS…'; get('verification-status').dataset.state = '';
    try {
      const response = await fetch(endpoint('verification'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}', signal: verificationRequest.signal });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || '测试未完成，请重试。');
      if (testingPage) renderVerification(result.result);
    } catch (cause) {
      if (testingPage) { get('verification-status').textContent = cause instanceof Error ? cause.message : '测试未完成，请重试。'; get('verification-status').dataset.state = 'failed'; }
    } finally { verificationBusy = false; get<HTMLButtonElement>('run-verification').disabled = false; get<HTMLButtonElement>('finish').disabled = false; }
  });
  function navigateAfterDeployment(destination: 'deployment' | 'access' | 'test') {
    clearCredentials(); verificationRequest?.abort(); clearTimeout(accessTimer);
    viewingDeployment = destination === 'deployment'; testingPage = destination === 'test';
    history.replaceState(null, '', location.pathname + (testingPage ? '#test' : ''));
    if (currentDeployment) renderDeployment(currentDeployment);
  }
  get('view-deployment').addEventListener('click', () => navigateAfterDeployment('deployment'));
  get('configure-access').addEventListener('click', () => navigateAfterDeployment('access'));
  get('back-access').addEventListener('click', () => navigateAfterDeployment('access'));
  get('to-verification').addEventListener('click', () => navigateAfterDeployment('test'));
  async function poll() {
    const serial = ++deploymentRequestSerial;
    try {
      const response = await fetch(endpoint('deployment'));
      if (!response.ok) throw new Error();
      const result = await response.json() as Status;
      if (serial === deploymentRequestSerial) renderDeployment(result);
    } catch {
      if (serial !== deploymentRequestSerial) return;
      get('deployment-message').textContent = '暂时无法读取部署状态，正在重新连接…';
      timer = setTimeout(() => { void poll(); }, 2000);
    }
  }
  async function deploymentControl(action: 'stop' | 'retry') {
    if (deploymentAction) return;
    deploymentAction = true; ++deploymentRequestSerial; clearTimeout(timer);
    if (currentDeployment) renderDeployment(currentDeployment);
    clearTimeout(timer);
    try {
      const response = await fetch(endpoint('deployment/' + action), { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(action === 'retry' ? { revision } : {}) });
      const result = await response.json() as { deployment: Status; error?: string };
      if (!response.ok) throw new Error(result.error || '操作失败，请重试。');
      viewingDeployment = false; testingPage = false;
      renderDeployment(result.deployment);
    } catch (cause) {
      get('deployment-message').textContent = cause instanceof Error ? cause.message : '无法连接本地部署服务。';
    } finally {
      deploymentAction = false;
      get<HTMLButtonElement>('retry-deployment').disabled = false;
      get<HTMLButtonElement>('stop-deployment').disabled = currentDeployment?.phase === 'stopping' && !currentDeployment.stopFailed;
      get<HTMLButtonElement>('edit').disabled = false;
      get<HTMLButtonElement>('finish').disabled = Boolean(currentDeployment && ['preparing', 'running', 'stopping'].includes(currentDeployment.phase));
      if (currentDeployment && ['preparing', 'running', 'stopping'].includes(currentDeployment.phase)) { clearTimeout(timer); timer = setTimeout(() => { void poll(); }, 500); }
    }
  }
  get('stop-deployment').addEventListener('click', () => { void deploymentControl('stop'); });
  get('retry-deployment').addEventListener('click', () => { void deploymentControl('retry'); });
  back.addEventListener('click', () => { if (!busy && step > 0) { step--; renderStep(); } });
  get('edit').addEventListener('click', () => {
    clearCredentials(); verificationRequest?.abort(); testingPage = false; history.replaceState(null, '', location.pathname);
    completedAccess = undefined; dnsFingerprint = ''; resetDns();
    clearTimeout(timer); clearTimeout(accessTimer); viewingDeployment = false;
    get('deployment').hidden = true; get('access-complete').hidden = true; get('verification').hidden = true; get('finish-actions').hidden = true; form.hidden = false; get('form-heading').hidden = false;
    step = 0; renderStep();
    void probeEnvironment();
  });
  get('finish').addEventListener('click', async () => {
    const button = get<HTMLButtonElement>('finish'); button.disabled = true;
    try {
      const response = await fetch(endpoint('finish'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
      if (!response.ok) throw new Error();
      button.textContent = '向导已关闭'; get('edit').hidden = true; get('retry-deployment').hidden = true;
      clearCredentials(); get('credentials-status').textContent = '向导已关闭，凭据已从页面清除。';
      for (const id of ['reload-credentials', 'run-verification', 'back-access', 'view-deployment']) get<HTMLButtonElement>(id).disabled = true;
      get('deployment-note').textContent = get('local-access-status').textContent = '本地部署向导已退出，你可以关闭此页面。';
      clearTimeout(accessTimer); get<HTMLButtonElement>('install-access').disabled = true;
    } catch { button.disabled = false; get('deployment-note').textContent = get('credentials-status').textContent = '无法关闭向导，请等待正在进行的操作完成后重试。'; }
  });
  form.addEventListener('submit', async event => {
    event.preventDefault(); if (busy || !validate()) return;
    if (step < titles.length - 1) { step++; renderStep(); return; }
    setBusy(true); next.textContent = topology() === 'single-k3d' ? '正在检查端口…' : '正在启动…'; error.hidden = true;
    try {
      const response = await fetch(endpoint('deploy'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ revision, slug: slug.value.trim(), apps: selectedApps(), deployment: target() }) });
      const result = await response.json() as Snapshot & { error?: string };
      if (!response.ok || !result.config) throw new Error(result.error || '启动失败，请检查配置。');
      revision = result.revision; viewingDeployment = false; testingPage = false; renderDeployment(result.deployment);
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
      cliHost = result.host;
      if (result.config) slug.value = result.config.slug;
      applyTarget(result.config?.deployment ?? result.defaults);
      renderApps(result.config?.apps ?? apps.filter(app => app.selected).map(app => app.id));
      get('loading').hidden = true; setBusy(false); renderStep();
      if (result.deployment.phase !== 'idle') renderDeployment(result.deployment);
      else void probeEnvironment();
    } catch (cause) { get('loading').hidden = true; showError(cause instanceof Error ? cause.message : '无法连接本地部署服务。'); }
  }
  void load();
}
