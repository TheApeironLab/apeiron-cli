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
    const topology = () => document.querySelector('[name="topology"]:checked').value;
    let step = 0;
    let busy = false;
    let revision = null;
    let apps = [];
    let timer;
    const endpoint = (name) => new URL('api/' + name, location.href).href;
    const error = get('error');
    form.addEventListener('input', () => { error.hidden = true; });
    form.addEventListener('change', () => { error.hidden = true; });
    const titles = ['设置部署环境', '设置组织', '配置模型', '选择要启用的应用'];
    const subtitles = ['检测当前主机，选择安装方式。', '设置团队标识与访问域名。', '选择快速与深度思考模型，测试后即可用于 Apeiron。', '准备所选应用的资源，然后创建集群并部署。'];
    const modelProvider = get('model-provider');
    const modelUrl = get('model-url'), modelKey = get('model-key');
    const modelFast = get('model-fast'), modelDeep = get('model-deep');
    const modelSkip = get('model-skip'), modelClear = get('model-clear-key');
    let savedModelUrl = '', savedModelKey = false, modelPassed = '', modelSerial = 0;
    let modelAbort;
    let modelChoices = [];
    let preferredFast = '', preferredDeep = '';
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
    function invalidateModels() { modelPassed = ''; modelSerial++; modelAbort?.abort(); get('model-list').disabled = false; get('model-status').textContent = ''; get('model-status').className = 'hint'; }
    for (const input of [modelUrl, modelKey, modelClear])
        input.addEventListener('input', () => { invalidateModels(); clearModelChoices(); });
    for (const select of [modelFast, modelDeep])
        select.addEventListener('change', () => { invalidateModels(); updateModelTest(); });
    modelProvider.addEventListener('input', invalidateModels);
    modelSkip.addEventListener('change', () => { invalidateModels(); get('model-settings').hidden = modelSkip.checked; });
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
                get('model-status').textContent = data.results.map((r) => `${r.mode === 'fast' ? '快速' : '深度思考'}：通过（${(r.elapsedMs / 1000).toFixed(1)} 秒）`).join('；');
            }
        }
        catch (cause) {
            if (serial === modelSerial) {
                get('model-status').className = 'error';
                get('model-status').textContent = cause instanceof Error ? cause.message : '模型请求失败。';
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
        get('local-ports').hidden = mode !== 'single-k3d';
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
        const nodes = mode === 'multi-k3s' ? target().installation.nodes : [];
        const candidates = mode === 'single-k3d' ? ['127.0.0.1'] : mode === 'multi-k3s' ? nodes.filter(node => node.role === 'server').map(node => node.address) : cliHost.addresses;
        if (!entryEdited)
            entryIp.value = mode === 'multi-k3s' && ha.checked ? '' : candidates[0] ?? '';
        entryIp.readOnly = mode === 'single-k3d';
        get('entry-field').hidden = mode === 'single-k3d';
        get('entry-addresses').replaceChildren(...candidates.map(address => new Option(address, address)));
        get('entry-label').textContent = published ? '节点 IP' : '入口 IP';
        get('entry-note').textContent = published ? '部署主机网卡上的固定 IP，供 K3s 使用；用户通过上方的公网入口访问。' : mode === 'multi-k3s' && ha.checked ? '填写已配置的稳定入口 IP（负载均衡或 VIP）；向导不会自动创建 VIP。' : mode === 'multi-k3s' ? '默认使用控制节点 IP，也可填写已配置的入口 IP；固定到单个节点不提供入口高可用。' : '从 CLI 主机网卡建议，请确认这是访问设备可达的固定 IP。';
    }
    function renderAccessGuide(access) {
        completedAccess = access;
        get('completed-dns-guide').textContent = access.local ? `将下载的完整 hosts 配置合并到运行 CLI 的这台机器。本机 K3d HTTPS 入口为 127.0.0.1:${access.httpsPort ?? 443}。下面是 Apeiron 和 IAM 的示例。` : '在内网 DNS 添加以下 A 记录，并让访问设备使用该 DNS。少量工作站也可使用下载的 hosts 配置。';
        get('dns-records').textContent = access.local
            ? `127.0.0.1 apeiron.${access.domain}\n127.0.0.1 iam.${access.domain}`
            : `*.${access.domain}    A    ${access.entryIp}\n${access.domain}      A    ${access.entryIp}`;
        get('dns-scope').textContent = `检测发起于 CLI 主机 ${cliHost.name || '当前主机'}。${access.local ? '检查 Apeiron 与 IAM 的本机解析。' : '检查 Apeiron、IAM 和随机子域名的泛解析。'}浏览器若在其他电脑上，需在那台电脑上配置解析与证书信任；此检查不代表 HTTPS 或应用已经可用。`;
        const fingerprint = JSON.stringify([access.domain, access.entryIp, access.local]);
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
                body: JSON.stringify({ domain: access.domain, entryIp: access.entryIp, local: access.local }), signal: dnsRequest.signal });
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
    for (const input of document.querySelectorAll('[name="topology"]'))
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
    function target() {
        const mode = topology();
        const paired = accessMode() === 'relay' ? selectedConnection() : undefined;
        const nodes = mode === 'multi-k3s' ? Array.from(get('node-results').querySelectorAll('.node-result')).map(row => ({
            host: row.dataset.host, name: row.querySelector('[data-field=name]')?.value.trim() ?? '',
            address: row.querySelector('[data-field=address]')?.value ?? '',
            role: row.querySelector('[data-field=role]')?.value,
        })) : [];
        return { installation: { topology: mode, domain: domain.value.trim(), entryIp: entryIp.value.trim(), httpPort: mode === 'single-k3d' ? Number(fields.httpPort.value) : 80, httpsPort: mode === 'single-k3d' ? Number(fields.httpsPort.value) : 443, ha: mode === 'multi-k3s' && ha.checked, ...connection(), nodes, ...(accessMode() !== 'private' ? { publicAccess: { mode: accessMode(), publicIp: paired?.publicIp ?? publicField('public-ip'), tunnelPort: paired?.tunnelPort ?? Number(publicField('tunnel-port')), ...(paired ? { pairingId: paired.id } : accessMode() === 'relay' ? { gateway: { host: publicField('gateway-host'), sshUser: publicField('gateway-user'), sshKey: publicField('gateway-key'), sshPort: Number(publicField('gateway-port')) } } : {}) } } : {}) },
            runner: mode === 'single-k3d' ? 'docker' : 'native',
            root: '', offline: offline.checked, bundleDir: offline.checked ? fields.bundleDir.value.trim() : '',
            environment: '', kubeconfig: '', workDir: '', image: 'chentu-lab' };
    }
    function applyTarget(value) {
        const install = value.installation;
        const published = install?.publicAccess;
        renderConnections(published?.pairingId ?? '');
        if (selectedConnection())
            connectionMessage(selectedConnection());
        get('manual-entry').open = Boolean(published?.gateway);
        document.querySelector(`[name="access-mode"][value="${published?.mode ?? 'private'}"]`).checked = true;
        for (const [id, text] of Object.entries({ 'public-ip': published?.publicIp ?? '', 'gateway-host': published?.gateway?.host ?? '', 'gateway-user': published?.gateway?.sshUser ?? '', 'gateway-key': published?.gateway?.sshKey ?? '', 'gateway-port': String(published?.gateway?.sshPort ?? 22), 'tunnel-port': String(published?.tunnelPort ?? 19444) }))
            get(id).value = text;
        document.querySelector(`[name="topology"][value="${install?.topology ?? 'single-k3d'}"]`).checked = true;
        offline.checked = Boolean(value.offline);
        get('online').checked = !offline.checked;
        fields.bundleDir.value = value.bundleDir;
        domain.value = install?.domain ?? '';
        customDomain.checked = Boolean(domain.value && domain.value !== defaultDomain());
        get('custom-domain').closest('details').open = customDomain.checked;
        entryIp.value = install?.entryIp ?? '';
        entryEdited = Boolean(entryIp.value);
        entryTopology = topology();
        fields.httpPort.value = String(install?.httpPort ?? 54320);
        fields.httpsPort.value = String(install?.httpsPort ?? 54321);
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
        get('step-label').textContent = `第 ${step + 1} 步 / 共 7 步`;
        get('step-title').textContent = titles[step];
        get('step-subtitle').textContent = subtitles[step];
        back.hidden = step === 0;
        next.textContent = step === titles.length - 1 ? '开始部署' : '下一步';
        if (focus)
            (step === 0 ? get('online') : step === 1 ? slug : step === 2 ? get('model-url') : document.querySelector('[name="app"]:not(:disabled)'))?.focus();
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
        return Array.from(document.querySelectorAll('[name="app"]:checked')).map(input => input.value);
    }
    function updateSelection() {
        get('selected-count').textContent = `已选择 ${selectedApps().length} 个应用（${apps.filter(app => app.required).length} 个必选）`;
    }
    function renderApps(selected) {
        const grid = get('app-grid');
        grid.replaceChildren();
        let previousGroup = '';
        apps.forEach(app => {
            const group = app.required ? '必选应用' : app.selected ? '可选应用 · 默认选中' : '其他可选应用 · 默认不选';
            if (group !== previousGroup) {
                const heading = document.createElement('h2');
                heading.className = 'app-group';
                heading.textContent = group;
                grid.append(heading);
                previousGroup = group;
            }
            const label = document.createElement('label');
            label.className = 'app-card';
            label.dataset.required = String(app.required);
            const input = document.createElement('input');
            input.type = 'checkbox';
            input.name = 'app';
            input.value = app.id;
            input.checked = app.required || selected.includes(app.id);
            input.disabled = app.required;
            input.addEventListener('change', updateSelection);
            const icon = document.createElement('span');
            icon.className = 'app-icon';
            icon.textContent = app.name.slice(0, 1);
            icon.setAttribute('aria-hidden', 'true');
            const copy = document.createElement('span');
            copy.className = 'app-copy';
            const name = document.createElement('strong');
            name.textContent = app.name;
            const description = document.createElement('span');
            description.textContent = app.description;
            copy.append(name, description);
            label.append(input, icon, copy);
            grid.append(label);
            if (app.required) {
                const badge = document.createElement('span');
                badge.className = 'required-badge';
                badge.textContent = '必选';
                label.append(badge);
            }
        });
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
            if (topology() === 'single-k3d' && (![fields.httpPort.value, fields.httpsPort.value].every(value => /^\d{1,5}$/.test(value) && Number(value) >= 1 && Number(value) <= 65535) || Number(fields.httpPort.value) === Number(fields.httpsPort.value) || Number(fields.httpPort.value) === 443 || Number(fields.httpsPort.value) === 80)) {
                showError('请填写不同的 HTTP / HTTPS 端口（1–65535）；HTTP 不能使用 443，HTTPS 不能使用 80。');
                return false;
            }
            if (offline.checked && !fields.bundleDir.reportValidity())
                return false;
            if (topology() === 'single-k3s' && (!machine || !supportsK3sHost(machine.os.name, machine.os.version, machine.hardware.architecture))) {
                showError(machine ? '单机 K3s 需要 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64。' : '请先完成当前主机的系统检测。');
                return false;
            }
            if (topology() === 'multi-k3s') {
                if (nodesFingerprint !== fingerprint() || nodeFacts.length < 2 || nodeFacts.some(node => node.error)) {
                    showError('请先成功检测至少 2 台 Ubuntu 节点。');
                    return false;
                }
                const nodes = target().installation.nodes;
                const count = nodes.filter(node => node.role === 'server').length;
                if (ha.checked ? count < 3 || count % 2 === 0 : count !== 1) {
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
        if (step === 0 && accessMode() !== 'private' && (topology() !== 'single-k3s' || offline.checked)) {
            showError('公网入口目前支持在线单机 K3s。');
            return false;
        }
        if (connectionBusy) {
            showError('连接管理正在进行，请稍后重试。');
            return false;
        }
        if (step === 0 && accessMode() === 'relay' && selectedConnection()?.state === 'revoked') {
            showError('当前连接已撤销，请重新配对。');
            return false;
        }
        if (step === 0 && accessMode() !== 'private' && (!publicField('public-ip') || (accessMode() === 'relay' && !selectedConnection() && !publicField('gateway-host')))) {
            showError('请配对公网入口，或填写高级 SSH 配置。');
            return false;
        }
        if (step === 0 && !validIp()) {
            showError('请确认部署环境中的入口 IPv4 地址。');
            return false;
        }
        if (step === 1) {
            if (!slug.reportValidity() || !domain.reportValidity())
                return false;
            if (!validDomain()) {
                showError('请填写有效的平台域名。');
                return false;
            }
        }
        if (step === 3 && apps.some(app => app.required && !selectedApps().includes(app.id))) {
            showError('请保留全部必选应用。');
            return false;
        }
        if (step === 2 && !modelSkip.checked && modelPassed !== modelFingerprint()) {
            showError('请先测试快速和深度思考模型，或选择稍后配置。');
            return false;
        }
        return true;
    }
    function renderDeployment(status) {
        pairedSelect.disabled = true;
        currentDeployment = status;
        form.hidden = true;
        get('form-heading').hidden = true;
        get('finish-actions').hidden = false;
        const access = status.phase === 'succeeded' ? status.access : undefined;
        const showTest = Boolean(access && !viewingDeployment && testingPage);
        const showAccess = Boolean(access && !viewingDeployment && !testingPage);
        get('deployment').hidden = showAccess || showTest;
        get('verification').hidden = !showTest;
        renderProgress(showTest ? 6 : showAccess ? 5 : 4);
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
        retryButton.hidden = !['failed', 'cancelled'].includes(status.phase);
        retryButton.disabled = deploymentAction;
        get('edit').disabled = deploymentAction;
        if (deploymentAction)
            get('finish').disabled = true;
        get('deployment-note').textContent = active ? '关闭或刷新页面不会停止部署。点击“停止部署”会终止部署进程，保留已有资源；已提交给集群的任务可能继续运行。' : status.phase === 'cancelled'
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
        get('credentials-status').textContent = '正在读取本次部署的初始管理员凭据…';
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
                body: JSON.stringify(action === 'retry' ? { revision } : {}) });
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
    get('retry-deployment').addEventListener('click', () => { void deploymentControl('retry'); });
    back.addEventListener('click', () => { if (!busy && step > 0) {
        step--;
        renderStep();
    } });
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
        get('deployment').hidden = true;
        get('access-complete').hidden = true;
        get('verification').hidden = true;
        get('finish-actions').hidden = true;
        form.hidden = false;
        get('form-heading').hidden = false;
        step = 0;
        renderStep();
        void probeEnvironment();
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
        if (step < titles.length - 1) {
            step++;
            renderStep();
            return;
        }
        setBusy(true);
        next.textContent = topology() === 'single-k3d' ? '正在检查端口…' : '正在启动…';
        error.hidden = true;
        try {
            const response = await fetch(endpoint('deploy'), { method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ revision, slug: slug.value.trim(), apps: selectedApps(), models: modelSkip.checked ? null : modelInput(), deployment: target() }) });
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
            next.textContent = '开始部署';
        }
    });
    async function load() {
        setBusy(true);
        try {
            const response = await fetch(endpoint('config'));
            if (!response.ok)
                throw new Error('无法读取配置，请检查终端并重新运行 apeiron init。');
            const result = await response.json();
            connections = result.connections ?? [];
            apps = result.apps;
            revision = result.revision;
            cliHost = result.host;
            if (result.config)
                slug.value = result.config.slug;
            if (result.gateway) {
                slug.value = result.gateway.slug;
                slug.readOnly = true;
            }
            const savedModel = result.config?.models;
            if (savedModel) {
                modelProvider.value = savedModel.provider || '';
                modelUrl.value = savedModel.baseUrl;
                preferredFast = savedModel.fast;
                preferredDeep = savedModel.deep;
                savedModelUrl = savedModel.baseUrl;
                savedModelKey = savedModel.hasApiKey;
                modelKey.placeholder = savedModelKey ? '已保存；留空保持不变' : '无鉴权服务可留空';
            }
            applyTarget(result.config?.deployment ?? result.defaults);
            renderApps(result.config?.apps ?? apps.filter(app => app.selected).map(app => app.id));
            get('loading').hidden = true;
            setBusy(false);
            renderStep();
            if (result.deployment.phase !== 'idle')
                renderDeployment(result.deployment);
            else
                void probeEnvironment();
        }
        catch (cause) {
            get('loading').hidden = true;
            showError(cause instanceof Error ? cause.message : '无法连接本地部署服务。');
        }
    }
    void load();
}
