// Self-contained: embedded in both the source CLI and compiled executable.
export function initWizard(supportsK3sHost) {
    const get = (id) => document.getElementById(id);
    const form = get('setup-form');
    const slug = get('slug');
    const next = get('next');
    const back = get('back');
    const offline = get('offline');
    const domain = get('domain');
    const customDomain = get('custom-domain');
    const entryIp = get('entry-ip');
    const ha = get('ha');
    const hosts = get('node-hosts');
    const fields = {
        httpPort: get('http-port'), httpsPort: get('https-port'),
        bundleDir: get('bundle-dir'),
        sshUser: get('ssh-user'), sshPort: get('ssh-port'), sshKey: get('ssh-key'),
    };
    let machine;
    let nodeFacts = [];
    let nodeSerial = 0;
    let deploymentAction = false;
    let deploymentRequestSerial = 0;
    let nodesFingerprint = '';
    let nodeRequest;
    const remote = () => document.querySelector('[name="location"]:checked').value === 'ssh';
    const topology = () => remote() ? (nodeHosts().length > 1 ? 'multi-k3s' : 'single-k3s') : document.querySelector('[name="topology"]:checked').value;
    let step = 0;
    let busy = false;
    let revision = null;
    // Load the application catalog from the selected distro package when leaving step two.
    let apps = [];
    let releaseCatalog = null, resourceBudget = null;
    let distro = 'minimal';
    let timer;
    const endpoint = (name) => new URL('api/' + name, location.href).href;
    const error = get('error');
    form.addEventListener('input', () => { error.hidden = true; });
    form.addEventListener('change', () => { error.hidden = true; });
    const titles = ['选择部署位置', '选择发行版', '共存配置与资源预算', '域名、访问方式与模型', '预检与部署摘要'];
    const subtitles = ['选择集群部署在哪里，并检测目标机器。', '选择要安装的 Distro。', '确认服务端分配的入口端口、宿主共存预检与资源估算。', '设置团队标识、访问入口与既有模型服务。', '核对配置与预检结果，然后开始安装。'];
    const TOTAL_STEPS = 8;
    let currentInstance = null;
    let configPath = '';
    let savedInstallation = null;
    let instancesAvailable = false;
    let instanceList = [];
    let lastPreflight = null;
    let preflightSerial = 0;
    let preflightPending = Promise.resolve();
    const modelProvider = get('model-provider');
    const modelUrl = get('model-url'), modelKey = get('model-key');
    const modelFast = get('model-fast'), modelDeep = get('model-deep');
    const modelClear = get('model-clear-key');
    let savedModelUrl = '', savedModelKey = false, modelPassed = '', modelSerial = 0;
    let modelAbort;
    let modelChoices = [];
    let preferredFast = 'qwen-plus', preferredDeep = 'qwen-plus';
    const updateModelTest = () => { get('model-test').disabled = !modelChoices.includes(modelFast.value) || !modelChoices.includes(modelDeep.value); };
    function clearModelChoices() {
        modelChoices = [];
        for (const select of [modelFast, modelDeep]) {
            select.replaceChildren(new Option('请先获取模型列表', ''));
            select.disabled = true;
        }
        updateModelTest();
    }
    const modelInput = () => ({ provider: modelProvider.value.trim(), baseUrl: modelUrl.value.trim().replace(/\/+$/, ''), fast: modelFast.value.trim(), deep: modelDeep.value.trim(),
        ...(savedModelKey && savedModelUrl === modelUrl.value.trim().replace(/\/+$/, '') && !modelKey.value && !modelClear.checked ? {} : { apiKey: modelClear.checked ? '' : modelKey.value.trim() }) });
    const modelFingerprint = () => JSON.stringify(modelInput());
    function renderModelTest(state, input, results = [], message = '') {
        const status = get('model-status');
        status.className = 'model-feedback';
        status.dataset.state = state;
        const heading = document.createElement('div'), icon = document.createElement('span'), symbol = document.createElement('span'), title = document.createElement('span');
        heading.className = 'model-feedback-heading';
        icon.className = 'model-feedback-icon';
        icon.setAttribute('aria-hidden', 'true');
        symbol.textContent = state === 'success' ? '✓' : state === 'error' ? '!' : '…';
        icon.append(symbol);
        title.textContent = state === 'success' ? '两种模式均测试通过' : state === 'error' ? '模型测试未通过' : '正在测试两种模式…';
        heading.append(icon, title);
        status.replaceChildren(heading);
        if (state === 'success') {
            const cards = document.createElement('ul');
            cards.className = 'model-result-grid';
            for (const result of results) {
                const card = document.createElement('li'), label = document.createElement('strong'), model = document.createElement('span'), duration = document.createElement('span');
                card.className = 'model-result-card';
                label.textContent = `${result.mode === 'fast' ? '快速' : '深度思考'}：通过`;
                model.textContent = input[result.mode];
                duration.textContent = `耗时 ${(result.elapsedMs / 1000).toFixed(1)} 秒`;
                card.append(label, model, duration);
                cards.append(card);
            }
            status.append(cards);
        } else {
            const detail = document.createElement('p');
            detail.className = 'model-feedback-detail';
            detail.textContent = state === 'error' ? message : '正在依次发送请求，请稍候。';
            status.append(detail);
        }
        status.scrollIntoView({ block: 'nearest' });
    }
    function invalidateModels() { modelPassed = ''; modelSerial++; modelAbort?.abort(); get('model-list').disabled = false; get('model-status').textContent = ''; get('model-status').className = 'hint'; delete get('model-status').dataset.state; }
    for (const input of [modelUrl, modelKey, modelClear])
        input.addEventListener('input', () => { invalidateModels(); clearModelChoices(); });
    for (const select of [modelFast, modelDeep])
        select.addEventListener('change', () => { invalidateModels(); updateModelTest(); });
    modelProvider.addEventListener('input', invalidateModels);
    async function checkModels(action) {
        invalidateModels();
        const serial = modelSerial;
        if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(modelProvider.value.trim())) {
            get('model-status').className = 'error';
            get('model-status').textContent = '请填写有效的 Provider 标识。';
            return;
        }
        if (!modelUrl.value.trim() || !modelUrl.reportValidity()) {
            get('model-status').textContent = '请填写模型 API 地址。';
            return;
        }
        if (action === 'test' && (!modelFast.value.trim() || !modelDeep.value.trim())) {
            get('model-status').textContent = '请选择两种模式的模型。';
            return;
        }
        const input = modelInput(), fingerprint = modelFingerprint();
        if (action === 'list') {
            preferredFast = modelFast.value || preferredFast;
            preferredDeep = modelDeep.value || preferredDeep;
            clearModelChoices();
        }
        modelAbort = new AbortController();
        get('model-list').disabled = get('model-test').disabled = true;
        get('model-status').textContent = action === 'list' ? '正在获取模型…' : '正在测试快速和深度思考…';
        if (action === 'test') renderModelTest('pending', input);
        try {
            const response = await fetch(endpoint('models/' + action), { method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ revision, models: { ...input, fast: input.fast || 'list', deep: input.deep || 'list' } }), signal: modelAbort.signal });
            const data = await response.json();
            if (serial !== modelSerial)
                return;
            if (!response.ok)
                throw new Error(data.error || '模型请求失败。');
            if (action === 'list') {
                if (!Array.isArray(data.models) || !data.models.length)
                    throw new Error('模型服务未返回可用模型，请检查 API 地址、Key 和模型权限。');
                modelChoices = data.models;
                for (const [select, preferred] of [[modelFast, preferredFast], [modelDeep, preferredDeep]]) {
                    select.replaceChildren(new Option('请选择模型', ''), ...modelChoices.map(id => new Option(id, id)));
                    select.disabled = false;
                    if (modelChoices.includes(preferred))
                        select.value = preferred;
                }
                get('model-status').textContent = `已获取 ${modelChoices.length} 个模型，请选择两种模式。`;
            }
            else {
                modelPassed = fingerprint;
                renderModelTest('success', input, data.results);
            }
        }
        catch (cause) {
            if (serial === modelSerial) {
                get('model-status').className = 'error';
                get('model-status').textContent = cause instanceof Error ? cause.message : '模型请求失败。';
                if (action === 'test') renderModelTest('error', input, [], get('model-status').textContent);
            }
        }
        finally {
            if (serial === modelSerial) {
                get('model-list').disabled = false;
                updateModelTest();
            }
        }
    }
    get('model-list').addEventListener('click', () => { void checkModels('list'); });
    get('model-test').addEventListener('click', () => { void checkModels('test'); });
    let probeRequest;
    let probeSerial = 0;
    let cliHost = { name: '', addresses: [] };
    let entryEdited = false;
    let entryTopology = '';
    let dnsFingerprint = '';
    let dnsSerial = 0;
    let dnsRequest;
    let completedAccess;
    let currentDeployment;
    let viewingDeployment = false;
    let testingPage = location.hash === '#test';
    let credentials;
    let credentialRequest;
    let credentialSerial = 0;
    let verificationRequest;
    let verificationBusy = false;
    let accessTimer;
    let accessInstalling = false;
    let connections = [];
    let connectionBusy = false;
    const pairedSelect = get('paired-entry');
    const selectedConnection = () => connections.find(connection => connection.id === pairedSelect.value);
    const accessMode = () => document.querySelector('[name="access-mode"]:checked').value;
    const publicField = (id) => get(id).value.trim();
    const defaultDomain = () => accessMode() === 'relay' && selectedConnection() ? selectedConnection().domain : /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(slug.value.trim()) ? `${slug.value.trim()}.${accessMode() === 'private' ? 'apeironlab.internal' : 'apeironlab.cn'}` : '';
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
        get('local-ports').hidden = mode !== 'single-k3d' || Boolean(currentInstance && !currentInstance.topology);
        if (entryTopology !== mode) {
            entryEdited = false;
            entryTopology = mode;
        }
        const paired = accessMode() === 'relay' ? selectedConnection() : undefined;
        if (paired) {
            domain.value = paired.domain;
            get('public-ip').value = paired.publicIp;
        }
        else if (!customDomain.checked)
            domain.value = defaultDomain();
        domain.readOnly = Boolean(paired) || !customDomain.checked;
        customDomain.disabled = Boolean(paired);
        get('public-ip').readOnly = Boolean(paired);
        get('public-dns').textContent = `*.${domain.value || '<slug>.apeironlab.cn'}    A    ${publicField('public-ip') || '<公网入口 IP>'}\n${domain.value || '<slug>.apeironlab.cn'}      A    ${publicField('public-ip') || '<公网入口 IP>'}`;
        const nodes = remote() ? target().installation.nodes : [];
        const candidates = mode === 'single-k3d' ? ['127.0.0.1'] : remote() ? nodes.filter(node => node.role === 'server').map(node => node.address) : cliHost.addresses;
        if (!entryEdited)
            entryIp.value = mode === 'multi-k3s' && ha.checked ? '' : candidates[0] ?? '';
        entryIp.readOnly = mode === 'single-k3d';
        get('entry-field').hidden = mode === 'single-k3d';
        get('entry-addresses').replaceChildren(...candidates.map(address => new Option(address, address)));
        get('entry-label').textContent = published ? '节点 IP' : '入口 IP';
        get('entry-note').textContent = published ? '部署主机网卡上的固定 IP，供 K3s 使用；用户通过上方的公网入口访问。' : mode === 'multi-k3s' && ha.checked ? '填写已配置的稳定入口 IP（负载均衡或 VIP）；向导不会自动创建 VIP。' : remote() ? '默认使用控制节点 IP，也可填写已配置的入口 IP；固定到单个节点不提供入口高可用。' : '从 CLI 主机网卡建议，请确认这是访问设备可达的固定 IP。';
    }
    function renderAccessGuide(access) {
        completedAccess = access;
        get('completed-dns-guide').textContent = access.local ? `将下载的完整 hosts 配置合并到运行 CLI 的这台机器。本机 K3d HTTPS 入口为 127.0.0.1:${access.httpsPort ?? 443}。下面是 Apeiron 和 IAM 的示例。` : access.private ? '管理机和集群节点已配置内部解析。浏览器在其他电脑上时，请合并下载的 hosts 配置，或在内网 DNS 添加以下记录。' : '在内网 DNS 添加以下 A 记录，并让访问设备使用该 DNS。少量工作站也可使用下载的 hosts 配置。';
        get('dns-records').textContent = access.local
            ? `127.0.0.1 apeiron.${access.domain}\n127.0.0.1 iam.${access.domain}`
            : `*.${access.domain}    A    ${access.entryIp}\n${access.domain}      A    ${access.entryIp}`;
        get('dns-scope').textContent = `检测发起于 CLI 主机 ${cliHost.name || '当前主机'}。${access.local || access.private ? '通过系统解析检查 Apeiron 与 IAM，包含 hosts 配置。' : '检查 Apeiron、IAM 和随机子域名的泛解析。'}浏览器若在其他电脑上，需在那台电脑上配置解析与证书信任；此检查不代表 HTTPS 或应用已经可用。`;
        const fingerprint = JSON.stringify([access.domain, access.entryIp, access.local, access.private]);
        if (dnsFingerprint !== fingerprint) {
            dnsFingerprint = fingerprint;
            resetDns();
        }
    }
    function resetDns() {
        dnsSerial++;
        dnsRequest?.abort();
        get('dns-results').replaceChildren();
        get('dns-status').textContent = '完成解析配置后，点击检测。';
        get('dns-status').dataset.state = '';
        get('copy-dns-status').textContent = '';
        get('check-dns').disabled = false;
    }
    for (const input of [slug, domain])
        input.addEventListener('input', renderSetup);
    customDomain.addEventListener('change', renderSetup);
    for (const input of document.querySelectorAll('[name="access-mode"]'))
        input.addEventListener('change', renderSetup);
    get('public-ip').addEventListener('input', renderSetup);
    function renderConnections(selected = pairedSelect.value) {
        pairedSelect.replaceChildren(new Option('选择入口', ''), ...connections.map(connection => new Option(`${connection.domain}${connection.state === 'revoked' ? '（已撤销）' : ''}`, connection.id)));
        pairedSelect.value = selected;
    }
    function connectionMessage(connection) {
        get('connection-status').textContent = connection.state === 'revoked' ? '已撤销公网连接；重新连接需要 ECS 生成新的配对码。' + (connection.localStopped === false ? '本机服务未停止，请使用 sudo 停止对应隧道服务；入口已拒绝重连。' : '') :
            `已配对 · ${connection.domain} → ${connection.publicIp}` +
                (connection.tunnel === undefined ? ' · 尚未检测隧道' : connection.tunnel ? ' · 隧道端口就绪' : ' · 隧道未连接') +
                (connection.routes === undefined ? '' : connection.routes ? ' · 路由已配置' : ' · 等待部署配置路由') +
                (connection.dns === undefined ? '' : connection.dns ? ' · DNS 通过' : ' · DNS 未通过') +
                (connection.https === undefined ? '' : connection.https ? ' · HTTPS 通过' : ' · HTTPS 未通过');
    }
    pairedSelect.addEventListener('change', () => {
        get('revoke-confirm').hidden = true;
        const selected = selectedConnection();
        if (selected)
            connectionMessage(selected);
        else
            get('connection-status').textContent = '';
        renderSetup();
    });
    async function connectionAction(action) {
        if (connectionBusy)
            return;
        const selected = selectedConnection();
        if (action !== 'pair' && !selected) {
            showError('请先选择已配对入口。');
            return;
        }
        const input = action === 'pair' ? { code: get('pairing-code').value.trim() } : { id: selected.id };
        if (action === 'pair')
            get('pairing-code').value = '';
        connectionBusy = true;
        get('connection-status').textContent = action === 'pair' ? '正在配对入口…' : action === 'revoke' ? '正在撤销公网连接…' : '正在检测入口…';
        const buttons = ['pair-entry', 'connection-refresh', 'connection-test', 'connection-revoke', 'confirm-revoke'].map(id => get(id));
        buttons.forEach(button => { button.disabled = true; });
        try {
            const response = await fetch(endpoint('connections/' + action), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(input) });
            const result = await response.json();
            if (!response.ok)
                throw new Error(result.error || '入口操作失败。');
            connections = result.connections;
            renderConnections(result.connection.id);
            connectionMessage(result.connection);
            get('revoke-confirm').hidden = true;
            renderSetup();
            if (currentDeployment?.phase === 'succeeded' && form.hidden)
                renderDeployment(currentDeployment);
        }
        catch (cause) {
            get('connection-status').textContent = '操作未确认成功；当前连接状态未知，请检查后重试。';
            showError(cause instanceof Error ? cause.message : '连接操作失败。');
        }
        finally {
            connectionBusy = false;
            buttons.forEach(button => { button.disabled = false; });
        }
    }
    get('pair-entry').addEventListener('click', () => { void connectionAction('pair'); });
    get('connection-refresh').addEventListener('click', () => { void connectionAction('status'); });
    get('connection-test').addEventListener('click', () => { void connectionAction('test'); });
    get('connection-revoke').addEventListener('click', () => { if (selectedConnection())
        get('revoke-confirm').hidden = false;
    else
        showError('请先选择已配对入口。'); });
    get('cancel-revoke').addEventListener('click', () => { get('revoke-confirm').hidden = true; });
    get('confirm-revoke').addEventListener('click', () => { void connectionAction('revoke'); });
    entryIp.addEventListener('input', () => { entryEdited = true; renderSetup(); });
    get('node-results').addEventListener('change', renderSetup);
    get('copy-dns').addEventListener('click', async () => {
        try {
            await navigator.clipboard.writeText(get('dns-records').textContent ?? '');
            get('copy-dns-status').textContent = '配置已复制';
        }
        catch {
            get('copy-dns-status').textContent = '请选中上面的配置并手动复制。';
        }
    });
    get('check-dns').addEventListener('click', async () => {
        const access = completedAccess;
        if (!access)
            return;
        const serial = ++dnsSerial;
        dnsRequest?.abort();
        dnsRequest = new AbortController();
        const button = get('check-dns');
        button.disabled = true;
        get('dns-status').textContent = '正在检测解析…';
        get('dns-status').dataset.state = '';
        try {
            const response = await fetch(endpoint('dns'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ domain: access.domain, entryIp: access.entryIp, local: access.local, ...(access.private ? { private: true } : {}) }), signal: dnsRequest.signal });
            const result = await response.json();
            if (serial !== dnsSerial)
                return;
            if (!response.ok)
                throw new Error(result.error || '无法完成解析检查。');
            get('dns-status').textContent = result.passed ? `CLI 主机 ${result.checkedFrom} 解析通过` : '解析未通过，请按配置说明检查 DNS / hosts。';
            get('dns-status').dataset.state = result.passed ? 'passed' : 'failed';
            get('dns-results').replaceChildren(...result.checks.map(check => {
                const row = document.createElement('li');
                row.textContent = `${check.host} → ${check.addresses.join(', ') || (check.status === 'timeout' ? '检测超时' : '未解析')} · ${check.status === 'matched' ? '匹配' : '未通过'}`;
                return row;
            }));
            if (!result.passed) {
                get('manual-access').open = true;
                get('dns-guide').open = true;
            }
        }
        catch (cause) {
            if (serial === dnsSerial) {
                get('dns-status').textContent = cause instanceof Error ? cause.message : '解析检查失败。';
                get('dns-status').dataset.state = 'failed';
            }
        }
        finally {
            if (serial === dnsSerial)
                button.disabled = false;
        }
    });
    function renderNetworkCards(checks, fallback = '检测中…') {
        const results = new Map(checks.map(check => [check.host, check]));
        const labels = { reachable: '已连接', 'http-error': '响应异常', 'dns-error': '解析失败', 'tls-error': '连接异常', timeout: '连接超时', unreachable: '无法连接', cancelled: '已取消' };
        for (const card of get('probe-cards').querySelectorAll('[data-host]')) {
            const check = results.get(card.dataset.host);
            card.dataset.state = check ? check.status === 'reachable' && check.httpStatus !== 404 ? 'reachable' : 'limited' : fallback === '检测中…' ? 'pending' : fallback === '检测失败' ? 'limited' : 'skipped';
            card.querySelector('.network-status').textContent = check ? check.status === 'reachable' && check.host === 'release-assets.githubusercontent.com' ? '域名可达' : labels[check.status] : fallback;
            card.querySelector('.network-latency').textContent = check ? `${check.elapsedMs} ms` : '—';
        }
    }
    async function probeEnvironment(refresh = false) {
        const serial = ++probeSerial;
        probeRequest?.abort();
        probeRequest = new AbortController();
        const button = get('probe-refresh');
        button.disabled = true;
        button.textContent = '正在检测…';
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
            if (!response.ok)
                throw new Error();
            const result = await response.json();
            if (serial !== probeSerial)
                return;
            machine = result.machine;
            renderTarget();
            get('probe-os').textContent = [result.machine.os.name, result.machine.os.version].filter(Boolean).join(' ');
            get('probe-kernel').textContent = '内核 ' + result.machine.os.kernel;
            const hardware = result.machine.hardware;
            get('probe-arch').textContent = hardware.architecture.toUpperCase();
            get('probe-cores').textContent = `${hardware.cores} 核`;
            get('probe-memory').textContent = `${hardware.memoryGiB} GiB`;
            get('probe-cpu').textContent = (hardware.cpu || 'CPU 型号未提供') + (hardware.architecture !== hardware.runtimeArchitecture ? ` · CLI 运行架构 ${hardware.runtimeArchitecture}` : '');
            const labels = { reachable: '探测点均可连接', limited: '部分探测点可连接', unreachable: '探测点均未通过', skipped: '离线模式 · 已跳过', cancelled: '探测已取消' };
            network.textContent = labels[result.network.status];
            network.dataset.state = result.network.status;
            renderNetworkCards(result.network.checks, result.network.status === 'skipped' ? '已跳过' : '已取消');
            const statuses = { reachable: '已连接', 'http-error': '已响应，HTTP 错误', 'dns-error': 'DNS 解析失败', 'tls-error': 'TLS 连接失败', timeout: '连接超时', unreachable: '连接失败', cancelled: '已取消' };
            for (const check of result.network.checks) {
                const row = document.createElement('li');
                const site = document.createElement('span');
                site.textContent = `${check.name} · ${check.host}`;
                const state = document.createElement('span');
                state.dataset.state = check.status === 'reachable' && check.httpStatus !== 404 ? 'reachable' : 'limited';
                state.textContent = `${statuses[check.status]}${check.httpStatus === undefined ? '' : ` · HTTP ${check.httpStatus}`} · ${check.elapsedMs} ms`;
                if (check.host === 'release-assets.githubusercontent.com') {
                    const detail = document.createElement('small');
                    detail.textContent = check.httpStatus === 404 ? '已收到服务器响应，但根路径没有资源。尚未验证具体安装包能否下载。' : '这里只检测下载域名；具体安装包能否下载，需要在获取安装包时验证。';
                    row.append(detail);
                }
                row.prepend(site, state);
                get('probe-sites').append(row);
            }
            get('probe-note').textContent = result.network.status === 'skipped' ? '离线模式下不发起外网探测。' : '结果仅代表上述站点；Google 可连接不代表所有海外网站可用。部署模式由你选择。';
            get('probe-time').textContent = '检测于 ' + new Date(result.checkedAt).toLocaleTimeString();
        }
        catch {
            if (serial !== probeSerial)
                return;
            network.textContent = '检测未完成';
            network.dataset.state = 'unreachable';
            renderNetworkCards([], '检测失败');
            get('probe-note').textContent = '无法读取本机探测结果，请重新检测或检查 CLI 是否仍在运行。';
        }
        finally {
            if (serial === probeSerial) {
                get('machine-probe').setAttribute('aria-busy', 'false');
                get('system-probe').setAttribute('aria-busy', 'false');
                button.disabled = false;
                button.textContent = '重新检测';
            }
        }
    }
    get('probe-refresh').addEventListener('click', () => { void probeEnvironment(true); });
    function showError(message) { error.textContent = message; error.hidden = false; }
    function renderTarget() {
        const mode = topology();
        get('node-section').hidden = !remote();
        get('local-runtime').hidden = remote();
        get('ha-fields').hidden = mode !== 'multi-k3s';
        get('deployment-mode').textContent = remote() ? `远端 SSH · ${nodeHosts().length} 台节点` : `本机 · ${mode === 'single-k3d' ? 'K3d' : 'K3s'}`;
        fields.bundleDir.required = offline.checked;
        get('bundle-fields').hidden = !offline.checked;
        get('resource-mode').textContent = offline.checked ? '检查本地应用及依赖包；缺少或校验失败时停止，不访问外网。' : '选择应用后检查实际安装资源，下载与校验通过后才会安装。';
        const supported = machine && supportsK3sHost(machine.os.name, machine.os.version, machine.hardware.architecture);
        get('topology-note').textContent = remote() ? '添加一台或多台机器的 SSH 地址或别名，并检测连接。节点数决定集群规模；目标机器信息显示在下方。' : mode === 'single-k3d' ? '在这台安装机的 Docker 中运行集群，需要 Docker 正在运行。' : machine && !supported ? '本机 K3s 需要 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64；也可以选择远端 SSH 主机。' : '在这台 Ubuntu 安装机上部署 K3s，需要 root 或免密 sudo。';
        renderSetup();
    }
    for (const input of document.querySelectorAll('[name="topology"], [name="location"]'))
        input.addEventListener('change', renderTarget);
    for (const input of document.querySelectorAll('[name="source"]'))
        input.addEventListener('change', () => { renderTarget(); void probeEnvironment(true); });
    function connection() { return { sshUser: fields.sshUser.value.trim(), sshKey: fields.sshKey.value.trim(), sshPort: Number(fields.sshPort.value) }; }
    function nodeHosts() { return hosts.value.trim().split(/[\s,]+/).filter(Boolean); }
    function fingerprint() { return JSON.stringify({ hosts: nodeHosts(), ...connection() }); }
    function invalidateNodes() {
        nodeSerial++;
        nodeRequest?.abort();
        nodesFingerprint = '';
        nodeFacts = [];
        renderTarget();
        get('node-results').replaceChildren();
        get('nodes-note').textContent = '连接设置已更改，请重新检测节点。';
        renderSetup();
        const button = get('detect-nodes');
        button.disabled = false;
        button.textContent = '自动检测节点';
    }
    for (const input of [hosts, fields.sshUser, fields.sshKey, fields.sshPort])
        input.addEventListener('input', invalidateNodes);
    function renderNodes(saved) {
        get('node-results').replaceChildren();
        nodeFacts.forEach((facts, index) => {
            const row = document.createElement('article');
            row.className = 'node-result';
            row.dataset.host = facts.host;
            const header = document.createElement('header');
            const title = document.createElement('strong');
            title.textContent = facts.host;
            const status = document.createElement('span');
            status.textContent = facts.error ? '需要处理' : '检测通过';
            header.append(title, status);
            row.append(header);
            const detail = document.createElement('p');
            detail.textContent = facts.os ? `${facts.os} ${facts.version} · ${facts.architecture} · ${facts.cores} 核 · ${facts.memoryGiB} GiB 内存 · ${facts.diskGiB} GiB 可用磁盘` : '未能连接';
            row.append(detail);
            if (facts.error) {
                const note = document.createElement('p');
                note.className = 'node-error';
                note.textContent = facts.error;
                row.append(note);
            }
            if (facts.name) {
                const controls = document.createElement('div');
                controls.className = 'node-controls';
                const previous = saved?.find(node => node.host === facts.host);
                const nameLabel = document.createElement('label');
                nameLabel.textContent = '节点名称';
                const name = document.createElement('input');
                name.type = 'text';
                name.dataset.field = 'name';
                name.value = previous?.name ?? facts.name;
                nameLabel.append(name);
                const addressLabel = document.createElement('label');
                addressLabel.textContent = '节点内网 IP';
                const address = document.createElement('select');
                address.dataset.field = 'address';
                for (const ip of facts.addresses)
                    address.add(new Option(ip, ip));
                if (previous && facts.addresses.includes(previous.address))
                    address.value = previous.address;
                else if (facts.addresses.includes(facts.host))
                    address.value = facts.host;
                addressLabel.append(address);
                const roleLabel = document.createElement('label');
                roleLabel.textContent = '节点角色';
                const role = document.createElement('select');
                role.dataset.field = 'role';
                role.add(new Option('控制节点', 'server'));
                role.add(new Option('工作节点', 'agent'));
                role.value = previous?.role ?? (index < (ha.checked ? 3 : 1) ? 'server' : 'agent');
                roleLabel.append(role);
                controls.append(nameLabel, addressLabel, roleLabel);
                row.append(controls);
            }
            get('node-results').append(row);
        });
        renderSetup();
    }
    ha.addEventListener('change', () => renderNodes());
    get('detect-nodes').addEventListener('click', async () => {
        const serial = ++nodeSerial;
        nodeRequest?.abort();
        nodeRequest = new AbortController();
        const requested = fingerprint();
        nodesFingerprint = '';
        const button = get('detect-nodes');
        button.disabled = true;
        button.textContent = '正在检测…';
        get('nodes-note').textContent = '通过 SSH 检测节点，每次最多并行检测 4 台。';
        try {
            const response = await fetch(endpoint('nodes'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: requested, signal: nodeRequest.signal });
            const result = await response.json();
            if (serial !== nodeSerial)
                return;
            if (!response.ok || !result.nodes)
                throw new Error(result.error || '节点检测失败。');
            nodeFacts = result.nodes;
            nodesFingerprint = requested;
            renderNodes();
            error.hidden = true;
            get('nodes-note').textContent = nodeFacts.some(node => node.error) ? '请处理节点提示后重新检测。' : '已生成节点配置，请确认内网 IP 和控制／工作角色。部署前会再次核验。';
        }
        catch (cause) {
            if (serial === nodeSerial)
                get('nodes-note').textContent = cause instanceof Error ? cause.message : '节点检测失败。';
        }
        finally {
            if (serial === nodeSerial) {
                button.disabled = false;
                button.textContent = '自动检测节点';
            }
        }
    });
    get('import-ssh').addEventListener('click', async () => {
        try {
            const response = await fetch(endpoint('ssh-aliases'));
            const result = await response.json();
            if (!response.ok || !result.aliases?.length)
                throw new Error('没有可导入的 SSH 别名，请手动填写地址。');
            hosts.value = result.aliases.join('\n');
            invalidateNodes();
            get('nodes-note').textContent = '已导入 SSH 别名。请保留要部署的机器，再点击自动检测。';
        }
        catch (cause) {
            get('nodes-note').textContent = cause instanceof Error ? cause.message : '无法读取 SSH 配置。';
        }
    });
    get('pick-bundle').addEventListener('click', async () => {
        const button = get('pick-bundle');
        button.disabled = true;
        try {
            const response = await fetch(endpoint('pick-bundle'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
            const result = await response.json();
            if (!response.ok)
                throw new Error(result.error || '请手动填写目录路径。');
            if (result.path)
                fields.bundleDir.value = result.path;
        }
        catch (cause) {
            showError(cause instanceof Error ? cause.message : '请手动填写目录路径。');
        }
        finally {
            button.disabled = false;
        }
    });
    // Entry ports are allocated by the server when an instance is created; the
    // page only displays them. Native K3s is fixed to 80/443.
    function localPorts() {
        if (currentInstance?.topology === 'single-k3d' && currentInstance.httpPort) return [currentInstance.httpPort, currentInstance.httpsPort];
        if (savedInstallation?.topology === 'single-k3d' && savedInstallation.httpPort) return [savedInstallation.httpPort, savedInstallation.httpsPort];
        return [54320, 54321];
    }
    function target() {
        distro = document.querySelector('[name="distro"]:checked').value;
        const mode = topology();
        const paired = accessMode() === 'relay' ? selectedConnection() : undefined;
        const nodes = remote() ? Array.from(get('node-results').querySelectorAll('.node-result')).map(row => ({
            host: row.dataset.host, name: row.querySelector('[data-field=name]')?.value.trim() ?? '',
            address: row.querySelector('[data-field=address]')?.value ?? '',
            role: row.querySelector('[data-field=role]')?.value,
        })) : [];
        return { installation: { distro, location: remote() ? 'ssh' : 'local', topology: mode, domain: domain.value.trim(), entryIp: entryIp.value.trim(), httpPort: mode === 'single-k3d' ? localPorts()[0] : 80, httpsPort: mode === 'single-k3d' ? localPorts()[1] : 443, ha: mode === 'multi-k3s' && ha.checked, ...connection(), nodes, ...(accessMode() !== 'private' ? { publicAccess: { mode: accessMode(), publicIp: paired?.publicIp ?? publicField('public-ip'), tunnelPort: paired?.tunnelPort ?? Number(publicField('tunnel-port')), ...(paired ? { pairingId: paired.id } : accessMode() === 'relay' ? { gateway: { host: publicField('gateway-host'), sshUser: publicField('gateway-user'), sshKey: publicField('gateway-key'), sshPort: Number(publicField('gateway-port')) } } : {}) } } : {}) },
            runner: mode === 'single-k3d' ? 'docker' : 'native',
            root: '', offline: offline.checked, bundleDir: offline.checked ? fields.bundleDir.value.trim() : '',
            environment: '', kubeconfig: '', workDir: '', image: 'chentu-lab' };
    }
    function applyTarget(value) {
        const install = value.installation;
        if (["minimal", "clinview"].includes(install?.distro))
            document.querySelector(`[name="distro"][value="${install.distro}"]`).checked = true;
        distro = document.querySelector('[name="distro"]:checked').value;
        const published = install?.publicAccess;
        renderConnections(published?.pairingId ?? '');
        if (selectedConnection())
            connectionMessage(selectedConnection());
        get('manual-entry').open = Boolean(published?.gateway);
        document.querySelector(`[name="access-mode"][value="${published?.mode ?? 'private'}"]`).checked = true;
        for (const [id, text] of Object.entries({ 'public-ip': published?.publicIp ?? '', 'gateway-host': published?.gateway?.host ?? '', 'gateway-user': published?.gateway?.sshUser ?? '', 'gateway-key': published?.gateway?.sshKey ?? '', 'gateway-port': String(published?.gateway?.sshPort ?? 22), 'tunnel-port': String(published?.tunnelPort ?? 19444) }))
            get(id).value = text;
        document.querySelector(`[name="location"][value="${install?.location ?? (install?.topology === 'multi-k3s' ? 'ssh' : 'local')}"]`).checked = true;
        document.querySelector(`[name="topology"][value="${install?.topology === 'single-k3d' ? 'single-k3d' : 'single-k3s'}"]`).checked = true;
        offline.checked = Boolean(value.offline);
        get('online').checked = !offline.checked;
        fields.bundleDir.value = value.bundleDir;
        domain.value = install?.domain ?? '';
        customDomain.checked = Boolean(domain.value && domain.value !== defaultDomain());
        get('custom-domain').closest('details').open = customDomain.checked;
        entryIp.value = install?.entryIp ?? '';
        entryEdited = Boolean(entryIp.value);
        entryTopology = topology();
        savedInstallation = install ?? null;
        fields.httpPort.value = String(localPorts()[0]);
        fields.httpsPort.value = String(localPorts()[1]);
        fields.sshUser.value = install?.sshUser ?? '';
        fields.sshKey.value = install?.sshKey ?? '';
        fields.sshPort.value = String(install?.sshPort ?? 22);
        ha.checked = install?.ha ?? false;
        hosts.value = install?.nodes.map(node => node.host).join('\n') ?? '';
        renderTarget();
    }
    function renderStep(focus = true) {
        pairedSelect.disabled = false;
        get('gateway-fields').insertBefore(get('connection-manager'), get('manual-entry'));
        renderSetup();
        error.hidden = true;
        document.querySelectorAll('[data-panel]').forEach((panel, index) => { panel.hidden = index !== step; });
        renderProgress(step);
        get('step-label').textContent = `第 ${step + 1} 步 / 共 ${TOTAL_STEPS} 步`;
        get('step-title').textContent = titles[step];
        get('step-subtitle').textContent = subtitles[step];
        back.hidden = step === 0;
        next.textContent = step === titles.length - 1 ? '开始安装' : '下一步';
        if (step === 1) updateSelection();
        if (step === 2) {
            renderCoexistence();
            renderBudget();
            void runPreflight();
        }
        if (step === 4) {
            renderSummary();
            void runPreflight();
        }
        if (focus)
            (step === 0 ? get('online') : step === 1 ? document.querySelector('[name="distro"]:checked') : step === 2 ? get('coexist-refresh') : step === 3 ? slug : next)?.focus();
    }
    function instanceLabel() {
        return currentInstance ? currentInstance.name : '实例未登记';
    }
    function renderChip() {
        get('instance-chip').textContent = instanceLabel();
        get('instance-chip').title = configPath ? `实例 ID：${currentInstance?.id ?? '未登记'}\n配置路径：${configPath}` : '当前正在操作的安装实例';
    }
    function renderCoexistence() {
        const mode = topology();
        get('instance-topology-label').textContent = remote() ? `远端 SSH · ${nodeHosts().length} 台节点` : `本机 · ${mode === 'single-k3d' ? 'K3d' : 'K3s'}`;
        get('coexist-instance').textContent = instanceLabel();
        get('coexist-config').textContent = configPath || '—';
        get('coexist-ports').textContent = mode === 'single-k3d' ? `HTTP ${localPorts()[0]} / HTTPS ${localPorts()[1]}（服务端分配）` : 'HTTP 80 / HTTPS 443（原生 K3s 固定）';
    }
    function renderBudget() {
        const selected = selectedApps();
        const memory = remote() ? nodeFacts.reduce((sum, node) => sum + (node.memoryGiB || 0), 0) : machine?.hardware?.memoryGiB;
        get('budget-host').textContent = remote() ? `${nodeFacts.length} 台目标机器 · 共 ${memory} GiB 内存` : machine ? `${machine.hardware.cores} 核 · ${memory} GiB 内存 · ${machine.os.name} ${machine.os.version}` : '尚未检测';
        const releases = releaseCatalog?.releases;
        const entries = releases?.filter(r => r.labels?.app === 'true') ?? [];
        get('budget-apps').textContent = releases ? `${releases.length} 个 release，其中 ${entries.length} 个应用入口：${entries.map(r => r.name).join('、')}` : `${selected.length} 个应用入口（旧安装包未提供完整 release 目录）`;
        get('budget-release-details').hidden = !releases;
        get('budget-releases').replaceChildren(...(releases ?? []).map(r => {
            const li = document.createElement('li');
            li.textContent = `${r.name}${r.labels?.app === 'true' ? ' · 应用入口' : ''}`;
            return li;
        }));
        const missing = resourceBudget?.missingMemoryRequests;
        const known = resourceBudget?.basis === 'helm-template' && resourceBudget.workloads > 0
            && Array.isArray(missing) && missing.length === 0
            && Number.isFinite(resourceBudget.memoryRequestsBytes) && Number.isFinite(resourceBudget.memoryRequestsBytesPerNode);
        if (known) {
            const nodes = remote() ? nodeFacts.length : 1;
            const gib = (resourceBudget.memoryRequestsBytes + nodes * resourceBudget.memoryRequestsBytesPerNode) / 1073741824;
            get('budget-memory').textContent = `Chart 声明的 memory requests：${gib.toFixed(2)} GiB`;
        } else {
            get('budget-memory').textContent = missing?.length ? `Chart 未完整声明 memory requests（${missing.length} 个容器缺失），暂不提供总内存估算。` : '安装包未提供完整的 Chart 资源声明，暂不提供总内存估算。';
        }
        get('budget-note').textContent = 'requests 是调度预留量，不代表实际占用或推荐主机容量；还需为操作系统、K3s/K3d 和安装迁移任务保留资源。';
        get('budget-note').dataset.state = '';
    }
    function renderPreflightResult(result) {
        for (const [statusId, listId] of [['coexist-status', 'coexist-warnings'], ['summary-preflight', 'summary-warnings']]) {
            const status = get(statusId);
            const list = get(listId);
            list.replaceChildren();
            if (!result) {
                status.textContent = '正在预检…';
                status.dataset.state = '';
                continue;
            }
            if (result.error) {
                status.textContent = `预检失败：${result.error}`;
                status.dataset.state = 'blocked';
                continue;
            }
            if (result.blocked) {
                status.textContent = `预检未通过：${result.blocked}`;
                status.dataset.state = 'blocked';
                continue;
            }
            const warnings = (result.preflight || '').split(/(?<=[。；])\s*/).map(text => text.trim()).filter(Boolean);
            status.textContent = (warnings.length ? `预检通过，有 ${warnings.length} 条提示。` : '预检通过，未发现共存冲突。') + (result.provisional ? ' 域名尚未填写，摘要步骤会按最终域名复查集群归属。' : '');
            status.dataset.state = warnings.length ? 'warning' : 'clear';
            list.replaceChildren(...warnings.map(text => { const li = document.createElement('li'); li.textContent = text; return li; }));
        }
    }
    function runPreflight() {
        const serial = ++preflightSerial;
        renderPreflightResult(null);
        // Aborting fetch leaves Docker inspection holding the server's request
        // gate. Drain it before the latest preflight or installation can start.
        preflightPending = preflightPending.catch(() => { }).then(() => {
            if (serial === preflightSerial)
                return preflightOnce(serial);
        });
        return preflightPending;
    }
    async function preflightOnce(serial) {
        try {
            // Before the domain step the cluster identity is unknown: check host
            // coexistence under a placeholder domain and recheck in the summary.
            const deployment = target();
            const provisional = !validDomain();
            if (provisional)
                deployment.installation.domain = 'preflight.apeironlab.internal';
            const response = await fetch(endpoint('preflight'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ revision, slug: slug.value.trim() || 'preflight', apps: selectedApps(), models: null, deployment }) });
            const result = await response.json();
            if (serial !== preflightSerial)
                return;
            lastPreflight = response.ok ? { ...result, provisional } : { error: result.error || '预检失败。' };
        }
        catch (cause) {
            if (serial !== preflightSerial)
                return;
            lastPreflight = { error: cause instanceof Error && cause.name !== 'AbortError' ? cause.message : '无法完成预检。' };
        }
        renderPreflightResult(lastPreflight);
    }
    get('coexist-refresh').addEventListener('click', () => { void runPreflight(); });
    get('summary-refresh').addEventListener('click', () => { void runPreflight(); });
    function renderSummary() {
        const value = target();
        const install = value.installation;
        get('summary-instance').textContent = instanceLabel();
        get('summary-topology').textContent = install.topology === 'single-k3d' ? '单机 K3d（本机 Docker）' : install.topology === 'multi-k3s' ? `多机 K3s${install.ha ? ' · 控制平面高可用' : ''}` : install.location === 'ssh' ? '单机 K3s（远端 SSH）' : '单机 K3s（当前主机）';
        get('summary-nodes').textContent = install.location === 'ssh' ? install.nodes.map(node => `${node.host}（${node.role === 'server' ? '控制' : '工作'} · ${node.address}）`).join('、') || '尚未检测' : install.topology === 'single-k3d' ? '本机 Docker 容器' : '当前主机';
        get('summary-source').textContent = (install.distro === 'clinview' ? 'Clinview' : 'Minimal') + ' · ' + (value.offline ? `离线 · ${value.bundleDir}` : '在线下载固定发行版');
        get('summary-domain').textContent = install.domain || '—';
        get('summary-entry').textContent = install.publicAccess ? `${install.publicAccess.mode === 'relay' ? 'ECS 转发' : '公网直连'} · ${install.publicAccess.publicIp}` : `内网 · ${install.entryIp || '—'}`;
        get('summary-ports').textContent = `HTTP ${install.httpPort} / HTTPS ${install.httpsPort}`;
        get('summary-apps').textContent = selectedApps().map(id => apps.find(app => app.id === id)?.name ?? id).join('、');
        get('summary-models').textContent = `${modelProvider.value.trim()} · ${modelUrl.value.trim()} · ${modelFast.value} / ${modelDeep.value}`;
    }
    function renderProgress(active) {
        document.querySelectorAll('[data-step]').forEach((item, index) => {
            item.dataset.state = index === active ? 'active' : index < active ? 'done' : 'pending';
            if (index === active)
                item.setAttribute('aria-current', 'step');
            else
                item.removeAttribute('aria-current');
        });
    }
    function selectedApps() {
        return apps.map(app => app.id);
    }
    function gpuInput() {
        return selectedApps().includes('gpustack') ? { host: get('gpustack-host').value.trim(), port: Number(get('gpustack-port').value) } : null;
    }
    function updateSelection() {
        const selected = selectedApps();
        get('model-routing-note').textContent = selected.includes('gateway')
            ? 'Apeiron 通过 Gateway 使用模型，供应商 Key 仅写入受保护配置与 Gateway Secret。'
            : 'Apeiron 直接连接模型服务，API Key 仅写入受保护配置与 Kubernetes Secret。';
        get('gpustack-settings').hidden = !selected.includes('gpustack');
    }
    function renderApps() {
        updateSelection();
    }
    function setBusy(value) {
        busy = value;
        get('fields').disabled = value;
        back.disabled = value;
        next.disabled = value;
    }
    function validate() {
        if (step === 0) {
            if (offline.checked && !fields.bundleDir.reportValidity())
                return false;
            if (!remote() && topology() === 'single-k3s' && (!machine || !supportsK3sHost(machine.os.name, machine.os.version, machine.hardware.architecture))) {
                showError(machine ? '单机 K3s 需要 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64。' : '请先完成当前主机的系统检测。');
                return false;
            }
            if (remote()) {
                if (nodesFingerprint !== fingerprint() || nodeFacts.length < 1 || nodeFacts.some(node => node.error)) {
                    showError('请先成功检测所选的 Ubuntu 节点（至少一台）。');
                    return false;
                }
                const nodes = target().installation.nodes;
                const count = nodes.filter(node => node.role === 'server').length;
                if (topology() === 'multi-k3s' && ha.checked ? count < 3 || count % 2 === 0 : count !== 1) {
                    showError(ha.checked ? '高可用需要至少 3 个、且为奇数个控制节点。' : '请选择 1 个控制节点，其余为工作节点。');
                    return false;
                }
                for (const key of ['host', 'name', 'address'])
                    if (new Set(nodes.map(node => node[key])).size !== nodes.length) {
                        showError('节点名称和地址不能重复。');
                        return false;
                    }
            }
        }
        if (step === 3 && accessMode() !== 'private' && (topology() !== 'single-k3s' || offline.checked)) {
            showError('公网入口目前支持在线单机 K3s。');
            return false;
        }
        if (connectionBusy) {
            showError('连接管理正在进行，请稍后重试。');
            return false;
        }
        if (step === 3 && accessMode() === 'relay' && selectedConnection()?.state === 'revoked') {
            showError('当前连接已撤销，请重新配对。');
            return false;
        }
        if (step === 3 && accessMode() !== 'private' && (!publicField('public-ip') || (accessMode() === 'relay' && !selectedConnection() && !publicField('gateway-host')))) {
            showError('请配对公网入口，或填写高级 SSH 配置。');
            return false;
        }
        if (step === 3 && !validIp()) {
            showError('请确认入口 IPv4 地址。');
            return false;
        }
        if (step === 3) {
            if (selectedApps().includes('gpustack') && (!get('gpustack-host').value.trim() || !get('gpustack-port').reportValidity() || !get('gpustack-port').value)) {
                showError('请填写 GPUStack 主机 IP 和服务端口。');
                return false;
            }
            if (!slug.reportValidity() || !domain.reportValidity())
                return false;
            if (!validDomain()) {
                showError('请填写有效的平台域名。');
                return false;
            }
        }
        if (step === 1 && apps.some(app => app.required && !selectedApps().includes(app.id))) {
            showError('请保留全部必选应用。');
            return false;
        }
        if (step === 1) {
            const missing = apps.find(app => selectedApps().includes(app.id) && app.requires.some(id => !selectedApps().includes(id)));
            if (missing) {
                showError(`${missing.name} 需要同时启用 ${missing.requires.map(id => apps.find(app => app.id === id)?.name ?? id).join('、')}。`);
                return false;
            }
        }
        if (step === 3 && modelPassed !== modelFingerprint()) {
            showError('请先配置并测试快速和深度思考模型。');
            return false;
        }
        return true;
    }
    function renderDeploymentStages(status) {
        const definitions = [
            ['check', '检查安装包', '核对部署配置、架构和所选应用的完整资源。'],
            ['cluster', '准备集群', '准备集群连接、网络、存储和入口。'],
            ['nexus', '启动制品仓库', '启动证书组件和 Nexus，配置仓库与访问凭据。'],
            ['import', '导入部署资源', '导入镜像、Chart 和部署资源，并实际读取校验。'],
            ['applications', '安装与初始化应用', '按依赖安装基础服务和所选应用，完成初始化。'],
            ['access', '准备访问与验证', '配置集群访问，整理地址和验证入口；随后测试登录。'],
        ];
        const labels = { pending: '等待', active: '进行中', done: '已完成', skipped: '未单独执行', failed: '失败', cancelled: '已停止' };
        const current = definitions.findIndex(([id]) => id === status.stage);
        const fallback = status.phase === 'preparing' ? 0 : status.phase === 'running' ? 4 : -1;
        const index = current < 0 ? fallback : current;
        get('deployment-stage-title').textContent = status.phase === 'succeeded'
            ? '部署阶段已完成'
            : index < 0 ? '安装阶段' : `阶段 ${index + 1} / ${definitions.length} · ${definitions[index][1]}`;
        get('deployment-stage-note').textContent = status.phase === 'succeeded'
            ? '下一步配置访问，并测试登录和应用功能。'
            : status.phase === 'failed' ? '本阶段未完成。修正原因后重新部署，后续阶段仍在等待。'
                : status.phase === 'cancelled' ? '部署已停止，已完成的阶段和集群资源保留。'
                    : '阶段进度跟随实际部署状态；刷新页面可以继续查看。';
        const previous = new Map([...get('deployment-stages').children].map(node => [node.dataset.stage, node]));
        const selected = index < 0 ? undefined : status.stages?.find(item => item.id === definitions[index][0]);
        get('deployment-current-stage').hidden = index < 0;
        get('deployment-current-stage').dataset.state = selected?.state || 'active';
        get('deployment-current-title').textContent = index < 0 ? '' : `${definitions[index][1]} · ${labels[selected?.state] || '进行中'}`;
        get('deployment-current-detail').textContent = selected?.detail || status.message || (index < 0 ? '' : definitions[index][2]);
        const items = definitions.map(([id, title, description], position) => {
            const saved = status.stages?.find(item => item.id === id);
            const state = saved && labels[saved.state] ? saved.state : position === index ? 'active' : 'pending';
            const item = previous.get(id) || document.createElement('li');
            item.dataset.stage = id;
            item.dataset.state = state;
            if (position === index && state !== 'done') item.setAttribute('aria-current', 'step');
            else item.removeAttribute('aria-current');
            if (!item.children.length) {
                const number = document.createElement('span'); number.className = 'deployment-stage-number'; number.setAttribute('aria-hidden', 'true');
                const copy = document.createElement('div'); copy.className = 'deployment-stage-copy';
                copy.append(document.createElement('strong'), document.createElement('p'));
                const badge = document.createElement('span'); badge.className = 'deployment-stage-state';
                item.append(number, copy, badge);
            }
            item.children[0].textContent = state === 'done' ? '✓' : String(position + 1);
            item.children[1].children[0].textContent = title;
            item.children[1].children[1].textContent = saved?.detail || description;
            item.children[2].textContent = labels[state];
            return item;
        });
        get('deployment-stages').replaceChildren(...items);
    }
    function renderDeployment(status) {
        pairedSelect.disabled = true;
        currentDeployment = status;
        form.hidden = true;
        get('form-heading').hidden = true;
        get('home').hidden = true;
        get('instances').hidden = true;
        get('finish-actions').hidden = false;
        const access = status.phase === 'succeeded' ? status.access : undefined;
        const showTest = Boolean(access && !viewingDeployment && testingPage);
        const showAccess = Boolean(access && !viewingDeployment && !testingPage);
        get('deployment').hidden = showAccess || showTest;
        get('verification').hidden = !showTest;
        renderProgress(showTest ? 7 : showAccess ? 6 : 5);
        const active = ['preparing', 'running', 'stopping'].includes(status.phase);
        get('deployment-title').textContent = status.phase === 'stopping' ? (status.stopFailed ? '停止尚未完成。' : '正在停止部署…') : active ? (status.completedRecovery ? '正在恢复访问配置…' : '正在部署…') : status.phase === 'succeeded' ? '部署完成。' : status.phase === 'cancelled' ? '部署已停止。' : status.phase === 'interrupted' ? '部署等待继续。' : '部署失败。';
        get('deployment-message').textContent = status.message;
        renderDeploymentStages(status);
        get('deployment-events').replaceChildren(...status.events.map(message => {
            const line = document.createElement('span');
            const level = /^\[(INFO|WARNING|ERROR)\]/.exec(message)?.[1] || 'INFO';
            line.className = `log-${level.toLowerCase()}`;
            line.textContent = message + '\n';
            return line;
        }));
        get('deployment-log').textContent = status.log || '正在检查安装资源';
        const openLog = get('open-log');
        const downloadLog = get('download-log');
        openLog.hidden = downloadLog.hidden = !status.log;
        openLog.href = endpoint('log');
        downloadLog.href = endpoint('log/download');
        get('deployment-environment').textContent = status.environment || '尚未生成';
        get('deployment-exit').textContent = status.exitCode === undefined ? '—' : String(status.exitCode);
        get('deployment-time').textContent = status.startedAt ? `${Math.max(0, Math.floor(((status.finishedAt ? Date.parse(status.finishedAt) : Date.now()) - Date.parse(status.startedAt)) / 1000))} 秒` : '—';
        get('finish').disabled = active || accessInstalling || verificationBusy;
        get('edit').hidden = active || showAccess || showTest;
        get('finish').hidden = Boolean(access && !showTest);
        get('view-deployment').hidden = !showAccess && !showTest;
        get('configure-access').hidden = !access || showAccess || showTest;
        get('to-verification').hidden = !showAccess;
        get('back-access').hidden = !showTest;
        const stopButton = get('stop-deployment');
        stopButton.hidden = !active;
        stopButton.disabled = deploymentAction || (status.phase === 'stopping' && !status.stopFailed);
        stopButton.textContent = status.stopFailed ? '重试停止' : status.phase === 'stopping' ? '正在停止…' : '停止部署';
        const retryButton = get('retry-deployment');
        retryButton.hidden = !['failed', 'cancelled', 'interrupted'].includes(status.phase);
        retryButton.disabled = deploymentAction;
        retryButton.textContent = status.completedRecovery ? '恢复访问配置' : status.resumable ? '继续原部署' : '重新部署';
        get('edit').disabled = deploymentAction;
        if (deploymentAction)
            get('finish').disabled = true;
        get('deployment-note').textContent = status.completedRecovery ? '核对原安装的配置、CLI、安装包、资源和集群身份后，恢复访问配置与登录测试。此操作不重新执行部署。' : status.resumable ? '仅继续已完成集群引导的同一次部署。将重新核对安装包、配置与集群身份，再执行同步；未完成引导的安装不会自动接管，升级新包需独立流程。' : active ? '关闭或刷新页面不会停止部署。点击“停止部署”会终止部署进程，保留已有资源；已提交给集群的任务可能继续运行。' : status.phase === 'cancelled'
            ? '点击“重新部署”使用上次保存的配置，检查资源和 Helm 状态后重新运行。不会断点续跑，也不会删除已有集群和数据。'
            : status.exitCode === undefined
                ? '安装在准备阶段停止。请根据日志修正原因后重试；已完成的变更不会自动回滚。'
                : '日志可在网页查看或下载。重试会重新检查资源并执行部署；已完成的变更不会自动回滚。';
        get('access-complete').hidden = !showAccess;
        if (access) {
            renderAccessGuide(access);
            for (const id of ['private-access-card', 'private-access-note', 'manual-access'])
                get(id).hidden = Boolean(access.public);
            get('completed-connection').hidden = !access.public || !selectedConnection();
            if (access.public && selectedConnection())
                get('completed-connection-slot').append(get('connection-manager'));
            get('access-complete').querySelector('.subtitle').textContent = access.public && selectedConnection()?.state === 'revoked' ? '公网连接已撤销，集群与数据保留。重新配对并部署入口后可恢复访问。' : access.public ? '公网 HTTPS 入口已验证。无需配置 hosts 或安装 CA，可以进入测试。' : '部署已完成。配置这台电脑的访问方式，然后进入测试。';
            get('access-entry').textContent = `https://apeiron.${access.domain}${(access.httpsPort ?? 443) === 443 ? '' : ':' + access.httpsPort} · ${access.public ? '公网访问' : access.local ? '本机测试' : '内网访问'} · ${access.entryIp}:${access.httpsPort ?? 443}`;
            get('access-notes').textContent = access.notes.join(' ');
            const ca = get('download-ca');
            ca.hidden = !access.ca;
            ca.href = endpoint('ca.crt');
            const hosts = get('download-hosts');
            hosts.hidden = !access.hostsPath;
            hosts.href = endpoint('hosts.txt');
            get('ca-path').textContent = access.ca?.path ?? '未生成';
            get('ca-fingerprint').textContent = access.ca?.fingerprint ?? '—';
            get('ca-expiry').textContent = access.ca ? new Date(access.ca.expiresAt).toLocaleDateString() : '—';
        }
        if (!access) {
            completedAccess = undefined;
            dnsFingerprint = '';
            resetDns();
        }
        clearTimeout(timer);
        if (active)
            timer = setTimeout(() => { void poll(); }, 1000);
        if (showAccess && !access?.public)
            void loadLocalAccess();
        if (showTest && access) {
            get('test-open-apeiron').href = `https://apeiron.${access.domain}${(access.httpsPort ?? 443) === 443 ? '' : ':' + access.httpsPort}/`;
            get('test-open-ops').href = `https://ops.${access.domain}${(access.httpsPort ?? 443) === 443 ? '' : ':' + access.httpsPort}/`;
            get('verification-scope').textContent = `检测发起于 CLI 主机 ${cliHost.name}，使用系统 DNS 与证书信任，直连部署入口。浏览器在其他电脑上时，需要在那台电脑上另行确认。`;
            if (!credentials)
                void loadCredentials();
            void fetch(endpoint('verification')).then(response => { if (!response.ok)
                throw new Error(); return response.json(); })
                .then(result => { if (result.result && testingPage)
                renderVerification(result.result); }).catch(() => { });
        }
    }
    function renderLocalAccess(result) {
        const { capability, status } = result;
        accessInstalling = status.phase === 'installing';
        const button = get('install-access');
        button.hidden = !capability.available;
        button.disabled = accessInstalling || !completedAccess?.ca || !completedAccess?.hostsPath;
        button.textContent = accessInstalling ? '正在配置…' : status.phase === 'succeeded' ? '重新配置本机访问' : status.phase === 'failed' || status.phase === 'cancelled' ? '重试配置本机访问' : '一键配置本机访问';
        get('local-access-host').textContent = `将配置 CLI 主机：${capability.host}`;
        get('local-access-description').hidden = !capability.available;
        get('local-access-status').textContent = !capability.available ? capability.reason : status.message || (!completedAccess?.ca || !completedAccess?.hostsPath ? '本次部署缺少访问文件，请查看下方说明。' : '系统会保留 hosts 原有条目，并在修改前备份。');
        get('local-access-status').dataset.state = status.phase;
        get('local-access-backup').textContent = status.backup ? `hosts 备份：${status.backup}` : '';
        get('local-access-checks').replaceChildren(...(status.checks ?? []).map(check => {
            const li = document.createElement('li');
            li.textContent = `${check.passed ? '✓' : '○'} ${check.host} · ${check.passed ? '解析与 HTTPS 通过' : '访问检查未通过'}`;
            return li;
        }));
        for (const id of ['finish', 'view-deployment', 'edit', 'to-verification'])
            get(id).disabled = accessInstalling;
        clearTimeout(accessTimer);
        if (accessInstalling)
            accessTimer = setTimeout(() => { void loadLocalAccess(); }, 1000);
    }
    async function loadLocalAccess() {
        try {
            const response = await fetch(endpoint('access'));
            if (!response.ok)
                throw new Error();
            renderLocalAccess(await response.json());
        }
        catch {
            get('local-access-status').textContent = '无法读取本机配置状态，请刷新页面重试。';
            if (accessInstalling)
                accessTimer = setTimeout(() => { void loadLocalAccess(); }, 2000);
        }
    }
    get('install-access').addEventListener('click', async () => {
        if (!completedAccess || accessInstalling)
            return;
        accessInstalling = true;
        get('install-access').disabled = true;
        for (const id of ['finish', 'view-deployment', 'edit', 'to-verification'])
            get(id).disabled = true;
        get('local-access-status').textContent = '正在请求 macOS 管理员授权…';
        try {
            const response = await fetch(endpoint('access/install'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
            const result = await response.json();
            if (!response.ok)
                throw new Error(result.error || '无法开始本机配置。');
            await loadLocalAccess();
        }
        catch (cause) {
            accessInstalling = false;
            for (const id of ['install-access', 'finish', 'view-deployment', 'edit', 'to-verification'])
                get(id).disabled = false;
            get('local-access-status').textContent = cause instanceof Error ? cause.message : '无法开始本机配置，请重试。';
        }
    });
    function clearCredentials() {
        credentialSerial++;
        credentialRequest?.abort();
        credentials = undefined;
        get('admin-username').value = '';
        get('admin-password').value = '';
        get('admin-password').type = 'password';
        get('toggle-password').textContent = '显示密码';
        get('toggle-password').setAttribute('aria-pressed', 'false');
        for (const id of ['copy-username', 'copy-password', 'toggle-password', 'download-credentials'])
            get(id).disabled = true;
    }
    async function loadCredentials() {
        clearCredentials();
        const serial = credentialSerial;
        credentialRequest = new AbortController();
        get('credentials-status').textContent = '正在读取本次部署的工作空间登录凭据…';
        get('reload-credentials').disabled = true;
        try {
            const response = await fetch(endpoint('credentials'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}', signal: credentialRequest.signal });
            const result = await response.json();
            if (!response.ok)
                throw new Error(result.error || '无法读取凭据，请重试。');
            if (serial !== credentialSerial || !testingPage)
                return;
            credentials = result;
            get('admin-username').value = result.username;
            get('admin-password').value = result.password;
            get('credentials-status').textContent = '凭据已就绪。密码默认隐藏，可显示或复制。';
            for (const id of ['copy-username', 'copy-password', 'toggle-password', 'download-credentials'])
                get(id).disabled = false;
        }
        catch (cause) {
            if (serial === credentialSerial)
                get('credentials-status').textContent = cause instanceof Error ? cause.message : '无法读取凭据，请重试。';
        }
        finally {
            if (serial === credentialSerial)
                get('reload-credentials').disabled = false;
        }
    }
    get('reload-credentials').addEventListener('click', () => { void loadCredentials(); });
    get('toggle-password').addEventListener('click', () => {
        const input = get('admin-password');
        const show = input.type === 'password';
        input.type = show ? 'text' : 'password';
        get('toggle-password').textContent = show ? '隐藏密码' : '显示密码';
        get('toggle-password').setAttribute('aria-pressed', String(show));
    });
    for (const key of ['username', 'password'])
        get('copy-' + key).addEventListener('click', async () => {
            if (!credentials)
                return;
            try {
                await navigator.clipboard.writeText(credentials[key]);
                get('credentials-status').textContent = `${key === 'username' ? '用户名' : '密码'}已复制。`;
            }
            catch {
                get('credentials-status').textContent = '无法访问剪贴板，请显示后选中对应字段复制。';
            }
        });
    get('download-credentials').addEventListener('click', () => {
        if (!credentials)
            return;
        const url = URL.createObjectURL(new Blob([JSON.stringify(credentials, null, 2) + '\n'], { type: 'application/json' }));
        const link = document.createElement('a');
        link.href = url;
        link.download = 'apeiron-admin.json';
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
    });
    function renderVerification(result) {
        get('verification-results').replaceChildren(...result.checks.map(check => {
            const li = document.createElement('li');
            const name = document.createElement('strong');
            name.textContent = `${check.name} · ${check.host}`;
            const message = document.createElement('span');
            message.textContent = check.message;
            message.dataset.state = check.https === 'passed' ? 'passed' : 'failed';
            const details = document.createElement('small');
            details.textContent = `解析${check.dns === 'passed' ? '通过' : '未通过'} · HTTPS ${check.https === 'passed' ? '通过' : check.https === 'skipped' ? '未检测' : '未通过'}`;
            li.append(name, message, details);
            return li;
        }));
        get('verification-status').textContent = `${result.passed ? '连接测试通过，请继续下方登录测试。' : '部分检查未通过，请返回配置访问或检查应用状态。'} 检测于 ${new Date(result.checkedAt).toLocaleTimeString()}`;
        get('verification-status').dataset.state = result.passed ? 'passed' : 'failed';
    }
    get('run-verification').addEventListener('click', async () => {
        if (verificationBusy)
            return;
        verificationBusy = true;
        verificationRequest = new AbortController();
        for (const id of ['run-verification', 'finish'])
            get(id).disabled = true;
        get('verification-status').textContent = '正在检查解析、证书信任和 HTTPS…';
        get('verification-status').dataset.state = '';
        try {
            const response = await fetch(endpoint('verification'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}', signal: verificationRequest.signal });
            const result = await response.json();
            if (!response.ok)
                throw new Error(result.error || '测试未完成，请重试。');
            if (testingPage)
                renderVerification(result.result);
        }
        catch (cause) {
            if (testingPage) {
                get('verification-status').textContent = cause instanceof Error ? cause.message : '测试未完成，请重试。';
                get('verification-status').dataset.state = 'failed';
            }
        }
        finally {
            verificationBusy = false;
            get('run-verification').disabled = false;
            get('finish').disabled = false;
        }
    });
    function navigateAfterDeployment(destination) {
        clearCredentials();
        verificationRequest?.abort();
        clearTimeout(accessTimer);
        viewingDeployment = destination === 'deployment';
        testingPage = destination === 'test';
        history.replaceState(null, '', location.pathname + (testingPage ? '#test' : ''));
        if (currentDeployment)
            renderDeployment(currentDeployment);
    }
    get('view-deployment').addEventListener('click', () => navigateAfterDeployment('deployment'));
    get('configure-access').addEventListener('click', () => navigateAfterDeployment('access'));
    get('back-access').addEventListener('click', () => navigateAfterDeployment('access'));
    get('to-verification').addEventListener('click', () => navigateAfterDeployment('test'));
    async function poll() {
        const serial = ++deploymentRequestSerial;
        try {
            const response = await fetch(endpoint('deployment'));
            if (!response.ok)
                throw new Error();
            const result = await response.json();
            if (serial === deploymentRequestSerial)
                renderDeployment(result);
        }
        catch {
            if (serial !== deploymentRequestSerial)
                return;
            get('deployment-message').textContent = '暂时无法读取部署状态，正在重新连接…';
            timer = setTimeout(() => { void poll(); }, 2000);
        }
    }
    async function deploymentControl(action) {
        if (deploymentAction)
            return;
        deploymentAction = true;
        ++deploymentRequestSerial;
        clearTimeout(timer);
        if (currentDeployment)
            renderDeployment(currentDeployment);
        clearTimeout(timer);
        try {
            const response = await fetch(endpoint('deployment/' + action), { method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(['retry', 'resume'].includes(action) ? { revision } : {}) });
            const result = await response.json();
            if (!response.ok)
                throw new Error(result.error || '操作失败，请重试。');
            viewingDeployment = false;
            testingPage = false;
            renderDeployment(result.deployment);
        }
        catch (cause) {
            get('deployment-message').textContent = cause instanceof Error ? cause.message : '无法连接本地部署服务。';
        }
        finally {
            deploymentAction = false;
            get('retry-deployment').disabled = false;
            get('stop-deployment').disabled = currentDeployment?.phase === 'stopping' && !currentDeployment.stopFailed;
            get('edit').disabled = false;
            get('finish').disabled = Boolean(currentDeployment && ['preparing', 'running', 'stopping'].includes(currentDeployment.phase));
            if (currentDeployment && ['preparing', 'running', 'stopping'].includes(currentDeployment.phase)) {
                clearTimeout(timer);
                timer = setTimeout(() => { void poll(); }, 500);
            }
        }
    }
    get('stop-deployment').addEventListener('click', () => { void deploymentControl('stop'); });
    get('retry-deployment').addEventListener('click', () => { void deploymentControl(currentDeployment?.resumable ? 'resume' : 'retry'); });
    back.addEventListener('click', () => { if (!busy && step > 0) {
        step--;
        renderStep();
    } });
    function showForm() {
        get('deployment-mode').parentElement.hidden = false;
        get('home').hidden = true;
        get('instances').hidden = true;
        get('deployment').hidden = true;
        get('access-complete').hidden = true;
        get('verification').hidden = true;
        get('finish-actions').hidden = true;
        form.hidden = false;
        get('form-heading').hidden = false;
        step = 0;
        renderStep();
        void probeEnvironment();
    }
    function showHome() {
        get('deployment-mode').parentElement.hidden = true;
        clearTimeout(timer);
        ++preflightSerial;
        form.hidden = true;
        get('form-heading').hidden = true;
        get('instances').hidden = true;
        get('deployment').hidden = true;
        get('access-complete').hidden = true;
        get('verification').hidden = true;
        get('finish-actions').hidden = true;
        get('home').hidden = false;
        get('manage-instances').disabled = !instancesAvailable;
        get('create-instance').disabled = !instancesAvailable;
        get('home-instance-count').textContent = instancesAvailable ? `注册表中有 ${instanceList.length} 个实例。` : '此向导不支持实例管理（网关向导或注册表不可用）。';
        renderProgress(-1);
        get('new-instance-name').focus();
    }
    async function loadInstances() {
        try {
            const response = await fetch(endpoint('instances'));
            const result = await response.json();
            if (!response.ok || !Array.isArray(result.instances))
                throw new Error(result.error || '无法读取实例列表。');
            instancesAvailable = true;
            instanceList = result.instances;
            get('instances-registry').textContent = result.registry;
            currentInstance = instanceList.find(instance => instance.id === result.current) ?? null;
            renderChip();
            return result;
        }
        catch (cause) {
            instancesAvailable = false;
            instanceList = [];
            get('instances-status').textContent = cause instanceof Error ? cause.message : '无法读取实例列表。';
            return null;
        }
    }
    function phaseLabel(instance) {
        const phases = { draft: '待选择部署位置', new: '未配置', idle: '已配置', interrupted: '安装中断，可继续', succeeded: '已部署', blocked: '恢复记录异常' };
        const k3d = { running: '集群运行中', stopped: '集群已停止', absent: '尚无集群', unknown: '集群状态未知' };
        return `${phases[instance.phase] ?? instance.phase}${instance.k3d ? ' · ' + (k3d[instance.k3d] ?? instance.k3d) : ''}${instance.locked ? ' · 被其他进程持有' : ''}`;
    }
    let pendingDelete = null;
    function renderInstances() {
        const list = get('instance-list');
        list.replaceChildren(...instanceList.map(instance => {
            const item = document.createElement('li');
            item.className = 'instance-card';
            item.dataset.id = instance.id;
            item.dataset.current = String(instance.id === currentInstance?.id);
            const header = document.createElement('header');
            const name = document.createElement('strong');
            name.textContent = instance.name;
            header.append(name);
            if (instance.id === currentInstance?.id) {
                const badge = document.createElement('span');
                badge.className = 'current-badge';
                badge.textContent = '当前操作的实例';
                header.append(badge);
            }
            const state = document.createElement('span');
            state.className = 'instance-state';
            state.dataset.state = instance.k3d === 'running' ? 'running' : instance.phase;
            state.textContent = phaseLabel(instance);
            header.append(state);
            const meta = document.createElement('div');
            meta.className = 'instance-meta';
            for (const [label, value] of [['ID', instance.id], ['部署类型', instance.topology || '待选择'], ['域名', instance.domain || '—'], ['端口', instance.httpPort ? `${instance.httpPort} / ${instance.httpsPort}` : '未分配'], ['配置', instance.config]]) {
                const cell = document.createElement('span');
                cell.textContent = label;
                const strong = document.createElement('b');
                strong.textContent = value;
                cell.append(strong);
                meta.append(cell);
            }
            const actions = document.createElement('div');
            actions.className = 'instance-actions';
            const select = document.createElement('button');
            select.type = 'button';
            select.className = 'button primary';
            select.textContent = instance.id === currentInstance?.id ? '继续配置' : instance.phase === 'interrupted' || instance.phase === 'succeeded' ? '选择并继续' : '选择';
            select.addEventListener('click', () => { void selectInstance(instance); });
            const diagnose = document.createElement('button');
            diagnose.type = 'button';
            diagnose.className = 'button';
            diagnose.textContent = '诊断采集';
            diagnose.title = '下载该实例当前部署的安装日志（api/log）';
            diagnose.addEventListener('click', () => { void collectDiagnostics(instance); });
            const remove = document.createElement('button');
            remove.type = 'button';
            remove.className = 'button';
            remove.textContent = '卸载';
            remove.disabled = instance.topology !== 'single-k3d' || instance.k3d === 'running' || instance.id === currentInstance?.id || instance.locked;
            remove.title = instance.topology !== 'single-k3d' ? '仅支持卸载 K3d 实例' : instance.k3d === 'running' ? '集群运行中，请先停止' : instance.id === currentInstance?.id ? '不能卸载当前实例' : '删除已停止的 K3d 集群与实例目录';
            remove.addEventListener('click', () => {
                pendingDelete = instance;
                get('instance-confirm-status').textContent = '';
                get('instance-confirm-target').textContent = `将卸载实例 ${instance.name}（${instance.id}）。`;
                get('instance-confirm').showModal();
            });
            actions.append(select, diagnose, remove);
            item.append(header, meta, actions);
            return item;
        }));
        if (!instanceList.length)
            get('instances-status').textContent = '注册表中还没有实例。返回首页创建新实例。';
    }
    async function showInstances() {
        get('home').hidden = true;
        get('instances').hidden = false;
        get('instance-confirm').close();
        get('instance-list').replaceChildren();
        get('instances-status').textContent = '正在读取实例…';
        await loadInstances();
        if (instancesAvailable)
            get('instances-status').textContent = '';
        renderInstances();
    }
    function applySelection(result) {
        revision = result.revision;
        configPath = result.path;
        currentInstance = result.instance;
        renderChip();
        const config = result.config;
        if (config)
            slug.value = config.slug;
        applyTarget(config?.deployment ?? { ...defaults, ...config?.setupSource });
        apps = [];
        releaseCatalog = resourceBudget = null;
    }
    async function selectInstance(instance) {
        get('instances-status').textContent = `正在切换到 ${instance.name}…`;
        try {
            if (instance.id !== currentInstance?.id) {
                const response = await fetch(endpoint('instances/select'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: instance.id }) });
                const result = await response.json();
                if (!response.ok)
                    throw new Error(result.error || '无法切换实例。');
                applySelection(result);
                if (result.deployment.phase !== 'idle') {
                    get('instances').hidden = true;
                    viewingDeployment = false;
                    testingPage = false;
                    renderDeployment(result.deployment);
                    return;
                }
            }
            else if (currentDeployment && currentDeployment.phase !== 'idle') {
                get('instances').hidden = true;
                renderDeployment(currentDeployment);
                return;
            }
            showForm();
        }
        catch (cause) {
            get('instances-status').textContent = cause instanceof Error ? cause.message : '无法切换实例。';
        }
    }
    async function collectDiagnostics(instance) {
        try {
            if (instance.id !== currentInstance?.id) {
                const response = await fetch(endpoint('instances/select'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: instance.id }) });
                const result = await response.json();
                if (!response.ok)
                    throw new Error(result.error || '无法切换实例。');
                applySelection(result);
                currentDeployment = result.deployment;
                renderInstances();
            }
            const response = await fetch(endpoint('log/download'));
            if (!response.ok)
                throw new Error('该实例尚未生成安装日志。');
            const url = URL.createObjectURL(await response.blob());
            const link = document.createElement('a');
            link.href = url;
            link.download = `install-${instance.name}.log`;
            link.click();
            setTimeout(() => URL.revokeObjectURL(url), 1000);
            get('instances-status').textContent = `已开始下载 ${instance.name} 的安装日志。`;
        }
        catch (cause) {
            get('instances-status').textContent = cause instanceof Error ? cause.message : '无法采集诊断。';
        }
    }
    get('instance-confirm').addEventListener('close', () => { pendingDelete = null; });
    get('instance-confirm').addEventListener('cancel', event => {
        if (get('instance-confirm-delete').disabled) event.preventDefault();
    });
    get('instance-cancel-delete').addEventListener('click', () => { get('instance-confirm').close(); });
    get('instance-confirm-delete').addEventListener('click', async () => {
        const instance = pendingDelete;
        if (!instance || get('instance-confirm-delete').disabled)
            return;
        get('instance-confirm-delete').disabled = true;
        get('instance-cancel-delete').disabled = true;
        get('instance-confirm-status').textContent = '正在卸载…';
        try {
            const response = await fetch(endpoint('instances/delete'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: instance.id }) });
            const result = await response.json();
            if (!response.ok)
                throw new Error(result.error || '无法卸载实例。');
            pendingDelete = null;
            get('instance-confirm').close();
            await loadInstances();
            renderInstances();
            get('instances-status').textContent = `已卸载 ${instance.name}${result.clusterDeleted ? '，并删除其 K3d 集群' : ''}。`;
        }
        catch (cause) {
            get('instance-confirm-status').textContent = cause instanceof Error ? cause.message : '无法卸载实例。';
        }
        finally {
            get('instance-confirm-delete').disabled = false;
            get('instance-cancel-delete').disabled = false;
        }
    });
    get('manage-instances').addEventListener('click', () => { void showInstances(); });
    get('instances-refresh').addEventListener('click', () => { void showInstances(); });
    get('instances-back').addEventListener('click', showHome);
    get('form-home').addEventListener('click', async () => {
        ++preflightSerial;
        await preflightPending.catch(() => { });
        await loadInstances();
        showHome();
    });
    get('configure-current').addEventListener('click', showForm);
    get('create-instance').addEventListener('click', async () => {
        const name = get('new-instance-name').value.trim();
        const status = get('create-instance-status');
        if (!/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(name)) {
            status.textContent = '请填写有效的实例名称。';
            return;
        }
        get('create-instance').disabled = true;
        status.textContent = '正在创建实例…';
        try {
            const response = await fetch(endpoint('instances/create'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ name, select: true }) });
            const result = await response.json();
            if (!response.ok)
                throw new Error(result.error || '无法创建实例。');
            if (result.path)
                applySelection(result);
            else {
                currentInstance = result.instance;
                renderChip();
            }
            status.textContent = result.existing ? `实例 ${name} 已登记，继续配置。` : `已创建实例 ${name}，请选择部署位置。`;
            await loadInstances();
            if (result.deployment && result.deployment.phase !== 'idle') {
                get('home').hidden = true;
                renderDeployment(result.deployment);
            }
            else
                showForm();
        }
        catch (cause) {
            status.textContent = cause instanceof Error ? cause.message : '无法创建实例。';
        }
        finally {
            get('create-instance').disabled = false;
        }
    });
    get('edit').addEventListener('click', () => {
        clearCredentials();
        verificationRequest?.abort();
        testingPage = false;
        history.replaceState(null, '', location.pathname);
        completedAccess = undefined;
        dnsFingerprint = '';
        resetDns();
        clearTimeout(timer);
        clearTimeout(accessTimer);
        viewingDeployment = false;
        showForm();
    });
    get('finish').addEventListener('click', async () => {
        const button = get('finish');
        button.disabled = true;
        try {
            const response = await fetch(endpoint('finish'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
            if (!response.ok)
                throw new Error();
            button.textContent = '向导已关闭';
            get('edit').hidden = true;
            get('retry-deployment').hidden = true;
            clearCredentials();
            get('credentials-status').textContent = '向导已关闭，凭据已从页面清除。';
            for (const id of ['reload-credentials', 'run-verification', 'back-access', 'view-deployment'])
                get(id).disabled = true;
            get('deployment-note').textContent = get('local-access-status').textContent = '本地部署向导已退出，你可以关闭此页面。';
            clearTimeout(accessTimer);
            get('install-access').disabled = true;
        }
        catch {
            button.disabled = false;
            get('deployment-note').textContent = get('credentials-status').textContent = '无法关闭向导，请等待正在进行的操作完成后重试。';
        }
    });
    form.addEventListener('submit', async (event) => {
        event.preventDefault();
        if (busy || !validate())
            return;
        if (step === 0) {
            setBusy(true);
            next.textContent = '正在读取安装包…';
            try {
                if (currentInstance) {
                    const configured = await fetch(endpoint('instances/target'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ revision, deployment: target() }) });
                    const selection = await configured.json();
                    if (!configured.ok) throw new Error(selection.error || '无法保存部署位置。');
                    revision = selection.revision;
                    currentInstance = selection.instance;
                    savedInstallation = selection.config.deployment.installation;
                    fields.httpPort.value = String(savedInstallation.httpPort);
                    fields.httpsPort.value = String(savedInstallation.httpsPort);
                    renderChip();
                }
            }
            catch (cause) {
                showError(cause instanceof Error ? cause.message : '无法读取安装包的应用目录。');
                return;
            }
            finally {
                setBusy(false);
            }
        }
        if (step === 1) {
            setBusy(true);
            next.textContent = '正在检查发行版…';
            try {
                const response = await fetch(endpoint('applications'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ deployment: target() }) });
                const result = await response.json();
                if (!response.ok) throw new Error(result.error || '无法读取发行版安装包。');
                if (result.distro !== distro) throw new Error('所选发行版与安装包不匹配，请返回部署选项选择对应的安装包。');
                if (result.releaseCatalog) {
                    if (result.releaseCatalog.schemaVersion !== 1 || !result.releaseCatalog.releases.length
                        || result.apps.length !== result.releaseCatalog.releases.length
                        || result.apps.some(app => app.releases.length !== 1 || app.releases[0] !== app.id))
                        throw new Error('安装包的 release 目录不完整。');
                } else {
                    const roots = [...result.apps].map(app => app.id).sort();
                    const expected = distro === 'clinview' ? ['apeiron', 'clinview', 'nexus', 'vasi'] : ['apeiron', 'nexus', 'vasi'];
                    if (JSON.stringify(roots) !== JSON.stringify(expected) || result.apps.some(app => !app.required))
                        throw new Error('发行版安装包的应用清单不完整，请使用匹配的发行包。');
                }
                apps = result.apps;
                releaseCatalog = result.releaseCatalog ?? null;
                resourceBudget = result.resourceBudget ?? null;
                renderApps();
            } catch (cause) {
                apps = [];
                releaseCatalog = resourceBudget = null;
                updateSelection();
                showError(cause instanceof Error ? cause.message : '无法检查发行版安装包。');
                return;
            } finally { setBusy(false); next.textContent = '下一步'; }
        }
        if (step < titles.length - 1) {
            step++;
            renderStep();
            return;
        }
        setBusy(true);
        next.textContent = topology() === 'single-k3d' ? '正在检查端口…' : '正在启动…';
        // The server serialises preflight and deploy; wait for an in-flight preflight instead of racing it.
        await preflightPending.catch(() => { });
        error.hidden = true;
        try {
            const response = await fetch(endpoint('deploy'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ revision, slug: slug.value.trim(), apps: selectedApps(), models: modelInput(), gpustack: gpuInput(), deployment: target() }) });
            const result = await response.json();
            if (!response.ok || !result.config)
                throw new Error(result.error || '启动失败，请检查配置。');
            revision = result.revision;
            viewingDeployment = false;
            testingPage = false;
            renderDeployment(result.deployment);
        }
        catch (cause) {
            showError(cause instanceof Error ? cause.message : '无法连接本地部署服务，请检查终端。');
        }
        finally {
            setBusy(false);
            next.textContent = '开始安装';
        }
    });
    let defaults;
    async function load() {
        setBusy(true);
        try {
            const response = await fetch(endpoint('config'));
            if (!response.ok)
                throw new Error('无法读取配置，请检查终端并重新运行 apeiron init。');
            const result = await response.json();
            connections = result.connections ?? [];
            revision = result.revision;
            cliHost = result.host;
            defaults = result.defaults;
            configPath = result.path ?? '';
            currentInstance = result.instance ?? null;
            renderChip();
            if (result.config)
                slug.value = result.config.slug;
            if (result.gateway) {
                slug.value = result.gateway.slug;
                slug.readOnly = true;
            }
            const savedModel = result.config?.models;
            if (result.config?.gpustack) {
                get('gpustack-host').value = result.config.gpustack.host;
                get('gpustack-port').value = result.config.gpustack.port;
            }
            if (savedModel) {
                modelProvider.value = savedModel.provider || '';
                modelUrl.value = savedModel.baseUrl;
                preferredFast = savedModel.fast;
                preferredDeep = savedModel.deep;
                savedModelUrl = savedModel.baseUrl;
                savedModelKey = savedModel.hasApiKey;
                modelKey.placeholder = savedModelKey ? '已保存；留空保持不变' : '无鉴权服务可留空';
            }
            applyTarget(result.config?.deployment ?? { ...result.defaults, ...result.config?.setupSource });
            get('loading').hidden = true;
            setBusy(false);
            if (result.deployment.phase !== 'idle') {
                renderStep(false);
                renderDeployment(result.deployment);
                return;
            }
            if (result.gateway) {
                showForm();
                return;
            }
            await loadInstances();
            showHome();
        }
        catch (cause) {
            get('loading').hidden = true;
            showError(cause instanceof Error ? cause.message : '无法连接本地部署服务。');
        }
    }
    void load();
}
