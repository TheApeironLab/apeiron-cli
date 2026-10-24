# Apeiron CLI

统一入口：`apeiron <模块> <命令>`。`apeiron init` 提供内置本地部署向导，`apeiron onto` 接入现有 Ontology CLI。Rust 单文件程序，内嵌浏览器向导；运行不需要 Bun/Node。onto 模块需要外部 CLI 或已安装依赖的 ontology 仓库。

## 安装发行版

macOS / Linux，ARM64 或 x64：

```sh
curl -fsSL https://downloads.apeironlab.cn/apeiron-cli/install.sh | sh
apeiron init
```

安装器自动选择平台并校验 SHA-256，优先原位更新已能找到的 `apeiron`，否则选择当前 PATH 中的常用 bin 目录。用户目录可直接写入；系统目录不可写时会请求 sudo 授权。安装后当前终端即可直接运行 `apeiron`，无需手动修改 PATH。`apeiron init` 自动打开默认浏览器。重新运行安装命令可升级 CLI，不会重新部署平台或修改配置。独立文件也可从 GitHub Releases（需要仓库权限）或公开 OSS 下载；离线安装和固定版本见 [发布与安装说明](docs/releases.md)。

新版安装器默认选择 `stable`，`latest.txt` 保留为 stable 的兼容入口。主动试用开发版或固定版本：

```sh
sh install.sh --channel nightly
sh install.sh --version 0.2.0
apeiron --version
apeiron --build-info
```

其中 `install.sh` 为下载到本地的安装脚本。nightly 按北京时间每日 03:00 检查 `main`，仅在有尚未成功发布的新提交时构建；不改变 stable、latest 或公共安装脚本。通道机制随首次正式发行启用；仓库版本设为 `0.2.0` 不表示线上已经发布。首次 stable 前，可使用某个已发布 nightly 目录内的安装脚本并显式指定版本，见发布文档。

CLI 可运行的平台与 Chentu 安装包支持的部署目标分别校验；当前预览包的全新部署目标为 **K3d ARM64**。Docker 等部署依赖仍需准备。

### 公网访问（在线单机 K3s）

有公网 IP 的主机选“公网直连”；内网主机选“ECS 转发”，填写公网 IP、已有 ECS 的 SSH 连接与 CLI 主机上的私钥路径。入口需预装 Caddy 2.10+ 并开放 80/443；SSH 需 root 或免密 sudo，并预先核验主机指纹。

Caddy 自动管理公网证书，ECS 转发使用由 systemd 管理的受限 SSH 反向隧道。集群内部保留原有 CA 与解析。公网域名与应用/SSO 域名一致；公网验证成功后，访问步骤不要求安装 CA 或修改 hosts。Nexus、registry、S3、监控内部端点及 IAM 管理路径保持内网访问；非 HTTP 服务不经该入口。

新功能需要同时更新 Chentu 发行包；缺少 `bootstrap/public_access.py` 的旧包会在创建集群前拒绝。在线主机检查不等于部署资源或公网 HTTPS 检查：真正的 Apeiron/IAM HTTPS 检查失败时，本次部署不会显示成功。此版本不自动购买 ECS、修改安全组或代管 DNS 账号。

## 源码开发安装

```sh
git clone https://github.com/TheApeironLab/apeiron-cli.git
cd apeiron-cli
cargo install --path runtime --locked
apeiron --help
```

## 统一 SSO 与 Task

```sh
apeiron sso login --server https://apeiron.example.internal
apeiron sso status --json
apeiron task workspace list --json
apeiron task project list --workspace <ID> --json
apeiron sso logout
```

`login` 显示设备授权链接并打开浏览器；浏览器复用已有 Keycloak SSO 会话，用户确认后 CLI
保存自己的平台登录。无桌面环境加 `--no-open`。后续 Task 命令从平台发现 API 地址，使用 SSO
凭据请求后端交换 Task 专用短期 token，不需要 Task 单独登录或逐 App 配置环境变量。

在 Apeiron agent／有执行超时的容器中使用两步登录，不让命令等待浏览器：

```sh
apeiron sso login --server https://apeiron.example.internal --no-open --no-wait --json
# 在自己的浏览器打开 verificationUri 并输入 userCode（链接已带 code 时无需重填），确认授权后：
apeiron sso login --complete --json
apeiron task workspace list --json
```

第一步立即返回 `authorizationPending: true`、授权链接和 `nextCommand`，它不表示登录已完成。
`--complete` 每次至多轮询一次；未确认或未到轮询间隔时返回 pending 和 `retryAfterMs`，稍后再执行。
设备授权状态和 PKCE verifier 保存在原容器的 0600 私有文件；不需要后台进程或复制 token。
网络故障导致换票结果不确定时要求重新发起登录，不重放可能已经消费的设备授权。
已有登录在新身份验证成功之前保留；`sso logout` 同时清理待完成的授权。

SSO refresh grant 只保存在当前用户的私有缓存，和网页的 refresh grant 分开；CLI 自动续期，
并发命令共用凭据锁。后端保管机密客户端密钥，只返回 App access token。刷新结果不确定时要求
重新登录，不反复提交可能已被消费的 refresh token。`logout` 只撤销当前 CLI 登录并清除派生缓存。

平台需要部署支持 `/sso/config`、`/sso/session`、`/sso/token` 的 Apeiron Backend 和 Chentu 的
public `apeiron-cli` 设备客户端。显式 SSO 登录后优先使用该会话；尚未登录的 agent 继续读取平台提供的外部凭据文件，不改写它。
旧的 `apeiron task auth login` 仅用于独立 Task 实例的兼容登录，不是平台 SSO 的前置步骤。
Task 是内置 Rust 模块，使用时不依赖 Node/Bun 或外部 Task 可执行文件。

### Task 项目、状态和子任务

先从 `apeiron task project list --workspace <工作区ID> --json` 获取项目的 `id`，不要传项目名称或 slug。

```sh
apeiron task project columns <项目ID> --json
apeiron task project fields <项目ID> --json
apeiron task create --project <项目ID> --title "任务" --status <列的slug> --json
apeiron task create --project <项目ID> --title "发布范围确定" --status <列的slug> --due 2026-10-26 --json
apeiron task update <任务ID> --due 2026-11-12T12:00:00+08:00 --json
apeiron task update <任务ID> --clear-due --json
apeiron task fields <任务ID> --json
apeiron task relation create --source <父任务ID> --target <子任务ID> --relation-type subtask --json
apeiron task relation list --task <任务ID> --json
apeiron task relation delete <关系ID> --json
apeiron task project archive <项目ID> --json
apeiron task project delete <项目ID> --confirm --json
```

`task columns --project <项目ID>` 也可查询列。`project fields` 返回字段定义，`task fields` 返回任务字段值；
创建任务时通过 `--field <字段ID>=<值>` 提交自定义字段。状态使用列的 `slug`，优先级使用
`no-priority/low/medium/high/urgent`。项目删除会永久删除项目内容，须显式传 `--confirm`。
创建和更新均支持 `--due`，写入原生 `dueDate` 字段。
`YYYY-MM-DD` 按当天 UTC 零点保存；精确时间必须带时区，例如 `2026-11-12T12:00:00+08:00`。
创建用 `--file` 时也可填写 `dueDate`。更新每次只改一个字段，`--clear-due` 清除截止日期。
子任务通过已有任务之间的 `subtask` 关系表达，父任务是 source，子任务是 target；创建任务与建关系是两个独立操作。
服务端当前没有独立的 milestone 模型，因此 CLI 不提供 `--milestone`，也不会把自定义字段伪装成原生里程碑。
这些新命令需要同时升级支持关系和项目删除的 Task Bearer 接口白名单；工作区权限仍由服务端检查。

## 浏览器部署向导

```sh
apeiron init
```

命令监听 `127.0.0.1` 的空闲端口，自动打开网页。首屏有两个入口：**创建新安装实例**（填写实例名称与拓扑，服务端分配入口端口并登记到 `~/.apeiron/instances.json`）和**管理已有安装实例**（列表显示 ID、名称、拓扑、域名、端口、状态；可选择／继续、采集诊断日志、卸载未运行的 K3d 实例）；页眉始终显示当前正在操作的实例。创建流程分为八步：

1. **拓扑**：依次展示系统配置、网络测试、部署选项；选择在线／离线、单机 K3s／单机 K3d／多机 K3s（发行包为每个目标声明支持的拓扑）。多机时展开节点连接与 SSH 检测。
2. **应用**：按“仅必选／核心协作／完整”场景组合快速选择，再按需增减；CI 需同时启用代码仓库。
3. **共存与预算**：只读展示服务端分配的入口端口，执行宿主共存预检（已有 K3s、k3d 容器、路由与 Docker 子网重叠、iptables），给出资源估算。
4. **域名与访问**：填写 Slug name，自动生成 `<slug>.apeironlab.internal`；可展开使用自定义域名。选择内网访问、公网直连或 ECS 转发（在线单机 K3s），填写入口 IP；配置必需的模型服务。
5. **摘要**：核对实例、拓扑、节点、来源、域名、入口、端口、应用、模型与预检结果，点击“开始安装”。
6. **部署**：完整准备并校验资源，创建集群，再运行宸途 `Helmfile sync`。显示进度、结果和日志入口。
7. **配置访问**：部署成功后自动进入。在有桌面会话的 Mac 上可点击“一键配置本机访问”；其他电脑使用折叠的手动指引。可以返回查看部署记录，不会再次部署。
8. **测试**：从访问配置点击“下一步：测试”。展示初始管理员用户名、默认隐藏的密码，提供显示／复制／下载，以及 Apeiron／Ops 登录入口。可以测试三处核心入口的解析和 HTTPS。

模型配置独立成页：预填百炼 `bailian` 标识和北京 Base URL `https://dashscope.aliyuncs.com/compatible-mode/v1`，填写同地域、同计费方案的 API Key；可修改为业务空间专属 OpenAI 兼容地址，获取模型列表，分别选择快速与深度思考模型，固定以 `apeiron-flash`（快速）和 `apeiron-pro`（深度思考）作为内部模型 ID，再发送简短请求测试（可能产生少量费用）。测试从 CLI 主机发起，部署后的集群也必须能访问模型服务；离线部署可使用内网模型端点。修改配置后须重新测试，基础安装不能跳过模型配置。Apeiron 通过必装的 Gateway 调用，供应商 Key 只写 Gateway Secret，Apeiron 使用网关签发的虚拟 Key。密钥仅保存于权限 0600 的本地配置及 Kubernetes Secret，不回传浏览器、不进入 Helm values。

基础安装的默认出口允许公开 IPv4 HTTPS 并排除私有网段；发行包已有代理或 CIDR 策略时保留它。内网模型或受限出口需在发行环境中声明实际代理/出口。地址依据 [百炼官方 Base URL 文档](https://help.aliyun.com/zh/model-studio/base-url)。需要包含 Gateway 镜像与依赖的新安装包，旧包不被自动补齐或覆盖。

基础模型向导的确定性浏览器验收：`APEIRON_TEST_BIN=/external/target/debug/apeiron bun scripts/test-base-models-ui.js`。测试拦截模型请求，不访问真实供应商。

| 选择规则 | 应用 |
| --- | --- |
| 必选，不可取消 | Nexus、Vasi、Apeiron（含 Ops）、Gateway |
| 默认不选 | Zygos、Limani、Task、Corpus、Chat、Files、邮件、Filer、代码仓库、GPUStack、Langfuse、Grafana、Browser（chrome-headless）、CI（woodpecker，需代码仓库） |

默认仅选中 4 项必选。Keycloak、数据库和存储由安装包中 Helmfile 的依赖闭包提供。页面按必选和可选分组。重新打开保留可选应用选择；旧配置中缺少的必选应用会自动勾选，提交时才写入。可选应用仍受发行包声明的部署依赖约束。

### 自动环境探测

页面依次展示两个独立区域：第一块“系统配置”显示操作系统、内核、架构、逻辑核心、内存和 CPU；第二块“网络测试”用 Microsoft、GitHub、Google、下载源四张卡片显示连接状态与耗时。完整域名、HTTP 状态码和网络说明放在默认折叠的“检测详情”中，异常结果也不会自动展开。图标随 CLI 内嵌，无外部图片请求。

第一步加载配置后，由 CLI 在本机检测操作系统（Linux 发行版/macOS/Windows）、硬件架构（AMD64/ARM64 等）、逻辑核心数与内存。macOS 上区分 ARM 硬件与 Rosetta 下运行的 x64 CLI。Linux 的运行时没有提供 CPU 名称时，回退到 `lscpu --json`，保留异构 CPU 的所有核心型号；仍无法识别时显示“CPU 型号未提供”。信息指运行 CLI 的主机，远程 K3s 节点不在此探测范围。

在线模式向 `www.microsoft.com`、`www.google.com`、`api.github.com`、`release-assets.githubusercontent.com` 发起并行 HTTPS HEAD 请求，每项超时 4 秒，不携带配置或访问令牌，不跟随重定向。Google 单独展示，用于检查该站点的海外访问情况，不代表所有海外网站都可用。结果区分已连接、已响应但 HTTP 错误、DNS/TLS 失败和超时；下载域名根路径返回 404 时，以提示颜色注明“根路径没有资源，尚未验证具体安装包能否下载”，不能据此判断安装包存在或拥有下载权限。结果仅代表固定探测点的连通性，不自动切换部署模式或阻止继续填写。

已保存的离线配置加载后不发起外网检测；页面切换到离线时取消正在执行的探测，仅显示本机信息。30 秒内复用最近结果，点击“重新检测”立即刷新。刷新检测不保存配置，也不连接 Kubernetes。

### 全新安装

第三块“部署选项”直接在页面内选择，不使用 CLI flags，也不提供“使用已有集群”。

| 方式 | 目标与限制 | 执行入口 |
| --- | --- | --- |
| 单机 K3s | 当前 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64 主机；需要匹配的原生安装包、root 或免密 sudo | 自动生成 local inventory → `bootstrap/hosts.yaml` → Helmfile |
| 单机 K3d | macOS / ARM64 + Docker；开发测试，使用发行包校验后的工具箱与镜像 | `tests/lab/helmfile.sh prepare` → `sync` |
| 多机 K3s | SSH 连接的 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64 节点；节点须匹配同一安装包，管理机可为 macOS | 校验并分发资源 → 自动生成 inventory → `bootstrap/hosts.yaml` → Helmfile |

普通多机至少 2 台，1 个控制节点加工作节点。控制平面高可用需至少 3 个、且为奇数个控制节点；不把“至少 3 台”强加给普通多机。SeaweedFS 使用 1 或 3 个存储节点；两节点部署使用 1 个 SeaweedFS 节点，Longhorn 副本数不超过节点数。这不等于所有应用都高可用。

节点区可导入 `~/.ssh/config` 的显式 Host 别名，或每行输入一个地址。点击“自动检测节点”后通过 SSH 读取系统、架构、CPU、内存、根磁盘可用空间、内网 IPv4、sudo 权限与已有 K3s 数据，并生成可调整的名称／IP／角色。修改连接参数后检测结果失效。使用已有 SSH 配置或 Agent，可另填私钥路径；不会读取私钥内容到网页。严格检查 known_hosts，首次连接需先用 SSH 确认指纹。最多 32 节点，每批并行 4 台。

安装前重新检测节点；多机还通过 Ansible 检查节点间 SSH 端口连通。K3s 端口尚未监听，不据此宣称防火墙已全面通过，实际加入由宸途检查。已有 K3s 数据会阻止全新安装，不自动清除或接管。域名 DNS、入口 VIP 和数据盘准备仍需完成；此版本不配置 DNS／VIP，不提供安装中断后的 K3s 接管或自动回滚。

部署环境步骤为 K3s 建议单机主机的网卡 IP 或普通多机的控制节点 IP；HA 模式需显式填写已配置的稳定入口 IP，向导不创建负载均衡或 VIP。原生私有安装使用包内 `bootstrap/private_dns.py`：按本次域名和入口 IP，在管理机及安装节点合并受管 hosts 区块，K3s 启动后合并本域 CoreDNS 配置；保留其他域的配置，冲突时停止。管理机须有 root 或免密 sudo 权限。浏览器在其他电脑上时，可合并下载的 hosts，或使用 `*.<domain> A <entryIp>` 内网 DNS；向导不修改其他工作站。部署完成页的解析检查由用户点击触发，从 CLI 主机的系统解析器发起（包含 hosts／macOS scoped resolver），私有安装通过系统解析检查 Apeiron、IAM（包含 hosts）；公网模式检查随机子域名。要求所有返回地址与入口 IP 一致，单项最多 4 秒。重新部署会作废旧结果，完成页使用本次成功部署的域名和入口 IP。K3s 安装前，Ansible 在管理机及节点配置受管 hosts 后，核验安装包声明的各服务名称解析；应用同步前配置集群内部解析。旧包缺少内部解析 helper 时在修改主机前拒绝。此检查不代表浏览器、Pod、TLS 或应用健康检查已经通过。

本机 K3d 默认使用 HTTP `54320`、HTTPS `54321`，可在部署环境中修改；应用 URL、登录回调与访问测试使用同一组端口，K3s 仍使用 80/443。点击开始部署时先检查端口：同一配置和域名持有的运行中集群按原端口重新部署，保留卷和凭据；其他服务占用、端口不匹配或 Docker 未启动时停留在配置页，不保存配置、下载资源或启动部署。创建集群前再次检查，避免检查后端口被抢占；不自动结束其他服务，也不接管无归属记录的集群。部署成功后的“配置访问”提供 Apeiron／IAM 的 hosts 示例和解析检测，本机模式只检测这两个名称，不要求 hosts 支持泛解析。部署成功后从宸途自有的 `chentu.lab.ingresshosts` 清单工具生成完整 hosts 文件。K3d 不在部署期间修改工作站 hosts 或 DNS；原生私有安装只配置 CLI 管理机和所选安装节点。部署后的 Mac 一键配置须由用户点击，合并本次域名的受管 hosts 区块，保留其他条目并备份原文件；检测到同名域名指向不同 IP 或不完整的区块时会停止，不覆盖冲突。手动配置仍需合并条目，不能覆盖原 hosts 文件。

部署成功后提供平台链接、公开 CA 证书下载、SHA-256 指纹、有效期，以及 macOS／Windows／Ubuntu 的信任指引。CA 由宸途签发，CLI 只读取本次部署导出的证书，检查有效期和 CA 属性，拒绝私钥、混合 PEM 与符号链接；文件接口仅开放固定的本次产物，不接受用户文件路径。证书或 hosts 提取失败会单独提示，不伪造下载文件，也不会把已成功的 Helmfile 同步改成失败。CA 私钥备份、根 CA 轮换和全节点信任分发仍遵循宸途现有流程，当前 CLI 未增加自动轮换。

Mac 一键配置使用 [AppleScript 系统管理员授权](https://developer.apple.com/library/archive/documentation/AppleScript/Conceptual/AppleScriptLangGuide/reference/ASLR_cmds.html)：密码只输入 macOS 弹窗，不发送给网页或 CLI。将**本次成功部署**的公开 CA 加入系统钥匙串，并仅设置 SSL 信任用途；在 `/private/etc/apeiron-access.*/hosts.before` 留下 hosts 备份。安装命令嵌入经过校验的证书及 hosts，不接受浏览器传入的路径、命令或证书；系统授权后再次核对 hosts 内容，避免覆盖期间发生的其他修改。取消授权可以重试，重复安装只替换当前域名的受管区块。操作期间禁止关闭向导、修改配置或重启部署；刷新网页只读取状态。

完成后从 CLI 主机检查 Apeiron／IAM 的系统解析和 HTTPS（使用系统信任、不跳过证书验证、不走环境代理），不把访问失败记成部署失败。浏览器独立信任库和代理仍可能需要单独设置；应让 `*.<平台域名>` 直连。页面始终标注实际被配置的 CLI 主机；非 macOS、SSH 或无桌面会话不提供一键按钮，可下载公开 CA 与 hosts 在浏览器所在电脑手动配置。不会把远程机器的系统配置误称为当前浏览器电脑已配置。

测试页只在成功部署后通过同源 POST 读取本次生成的 kubeconfig 所指向的 `keycloak/keycloak-bootstrap` Secret，仅返回 `username`、`password`。Docker 模式复用已安装工具箱镜像，以只读 kubeconfig 和集群网络查询，无 Docker socket、不拉取镜像。口令不进入进程参数、配置快照、状态接口或部署日志，也不自动写入本机凭据文件；用户点击下载时才生成文件。离开测试页／关闭向导清除页面密码，刷新后重新读取并恢复隐藏状态。初始 Secret 不会跟踪用户之后在 IAM 中修改的密码，因此页面明确说明其仅为首次安装凭据。

连接测试检查 Apeiron、Ops、IAM：从 CLI 主机解析地址，必须全部匹配部署入口，再使用系统证书信任发起 HTTPS HEAD。固定入口 IP、禁用环境代理、不跟随跳转、不会发送账号密码；HTTP 2xx／3xx 视为入口可访问，404 等错误或 TLS 失败不会通过。它不宣称用户已登录或应用功能全部正常；用户用显示的凭据打开应用完成登录测试。


离线模式可选择 CLI 主机上的目录；macOS 桌面可打开目录选择器，远程／无桌面环境直接填写绝对路径。离线不下载资源，缺少所选应用或依赖的文件、大小或 SHA-256 不符即停止。在线模式只下载缺失或损坏的所需文件，必须收到完整 HTTP 200 响应并通过大小与 SHA-256 校验；404、截断响应与校验失败不能放行集群创建。公共网站卡片不充当资源检查门槛。

**发行状态：CLI 固定使用兼容的 Chentu `0.1.0-rc.5` 预览包。** 包内提供 `deploymentTopology`、`clusterOidc`、自定义入口端口和 `setup/install.json`，按所选应用下载并校验资源。目前提供 `k3d-arm64` 目标，共 50 个资源文件，覆盖默认 9 个应用及可选 Gateway、Filer、代码仓库、Langfuse、Grafana；GPUStack 尚无对应资源，选择后会在修改集群前明确拒绝。

源码基础提交为 `f109f8567b9df0cf1eeb1b6d6cbd7092be5f99c3`，具体修复以包内 `chentu-package.json.sourceOverlay` 的逐文件摘要为准。安装包 SHA-256：`f45a9a370a15e01697ba82d8ad1101853c040a3b1bbf1c0e1f557164d4706021`。来源、验证与限制见 [release.json](https://downloads.apeironlab.cn/chentu/releases/0.1.0-rc.5/release.json)，消费契约见 [docs/install-package.md](docs/install-package.md)。已验证本机 ARM64 K3d 部署与 Vasi 集群 SSO 权限；原生 Ubuntu、AMD64、多节点和断网安装尚需分别验收。Apeiron 的真实 LLM 可在 setup 的模型步骤配置，开发模型桩不代表真实对话已可用。

环境 values、inventory、kubeconfig 由流程生成，不向用户索取。页面不再提供源码路径或开发选项：在线使用匹配版本的发行包，离线使用所选目录中的 `chentu/` 部署程序与应用资源。旧配置中的源码覆盖不会沿用到全新安装。仅 CLI 开发联调可显式设置进程变量 `APEIRON_CHENTU_ROOT`，该变量不写入安装配置，离线模式也不会使用它。

K3d 安装时，CLI 还会准备并校验与 K3s 版本匹配的官方 ARM64 基础镜像包（约 231 MiB），节点启动时直接导入 Traefik、CoreDNS 等镜像。基础镜像计入资源准备进度；Traefik 就绪后才安装应用。准备期间每 15 秒报告等待时长与基础服务状态，并记录镜像拉取事件。可选择离线 K3d 并提供完整本地安装包；完整应用的断网部署尚需验收。

### 配置与进度

每次部署在配置目录的 `deployments/run-*/` 生成 `environment.yaml`、`inventory.yaml`（K3s）与 `install.log`。
生成器以发行包声明为基础写入组织、域名、节点与存储规模，以及应用 `releases.<id>.enabled`。Helmfile 仍负责 release 默认值与 topology 预设合并、依赖校验、安装顺序和 hooks；资源清单仅负责文件选择与下载。

部署模式同时决定 Nexus 公网代理：在线部署写入 `releases.nexus.values.publicProxies: true`，启用 Chentu 已有的 Docker／npm 代理；离线部署明确写入 `false`。重新生成配置时以本次部署模式为准，保留 Nexus 的其他配置。自定义上游仍遵循 Chentu 的优先级规则；这是仓库配置，不是集群出口防火墙策略。

Limani 对应 `ontology`，Chat 对应 `matrix`，邮件对应 `stalwart`，代码仓库对应 `git`。Grafana 选项控制宸途 observability 组件的 `kps`、`loki`、`promtail` 三个 release；其余 ID 为小写应用名。Apeiron 包含 Ops，无需单独选择。
取消选择只将相应 release 排除于本次同步，不卸载已有应用；平台访问权限仍由平台管理。

网页展示部署状态、耗时、退出码、已识别的同步进度与日志路径。准备阶段的进度与错误也写入日志，失败时不会只留下空文件。“在网页打开日志”以纯文本展示本次运行的完整日志，刷新日志页可查看最新内容；“下载日志”保存同一文件。日志仍保存在 CLI 主机，使用 `0600` 权限；访问须经过本次向导的随机令牌和来源检查，接口不接受文件路径，拒绝符号链接。每次重试生成独立日志，页面链接随当前运行更新。完整输出可能含敏感诊断，不写入常规状态响应或终端；保管好向导链接及下载的日志。
部署页横向展示六个阶段：检查安装包、准备集群、启动制品仓库、导入部署资源、安装与初始化应用、准备访问与验证；当前执行内容显示在阶段条下方。阶段状态来自实际部署事件，未执行的引导或导入阶段显示“无需执行”，失败或停止保留所在阶段，后续步骤保持等待。阶段完成后继续配置本机访问并测试登录，不将安装成功等同于业务验收通过。部署日志默认折叠，可随时展开。
重复启动会被阻止，刷新页面会恢复当前进度。同一源环境文件的多个向导共享部署锁。
第 4 步提供“停止部署”。停止资源下载或本机部署进程，并清理本次运行的临时工具箱；不会删除集群、卷或回滚已完成的变更。已提交给 Kubernetes 的任务可能继续运行。停止期间禁用修改和重新部署；清理失败会保留部署锁，检查 Docker／目录权限后可“重试停止”。关闭、刷新页面不会发起停止。

正式 Nexus-first 安装成功后保存私有完成记录。重启同一个 CLI 后，可点击“恢复访问配置”：重新核对配置、安装包、资源和集群身份，再生成访问资料并进入访问／测试步骤，不再次执行同步；身份变化时拒绝恢复。新部署开始时撤销上次完成记录，原部署目录与日志仍保留。

正式 Nexus-first 安装完成集群引导并生成恢复检查点后，停止、失败或 CLI 进程重启可点击“继续原部署”：核对同一 CLI、配置、安装包、资源与集群身份，保留原目录和日志，重新检查并同步应用，hooks 可能重复执行。不会按进度条跳过操作，也不会把新版本包作为原安装继续。引导尚未完成的硬退出不自动接管，旧进程或子进程仍存活时拒绝并发继续。没有检查点的正常失败仍可点击“重新部署”，重新检查资源并生成独立日志。配置在其他窗口或进程中变化后，必须返回修改确认。每次 sync 前通过当前集群的 Helm 元数据检查 pending-install／pending-upgrade／pending-rollback；检测到未完成操作或读取失败时，阻止本次 sync，提示涉及的 release，不自动删除 Helm 记录或回滚。

管理机基础依赖在下载部署包前检查，缺失工具集中列出。原生应用需要可运行的 curl：可使用 CLI 主机已有工具或已校验离线包提供的工具，创建集群前执行版本检查；向导不会联网安装依赖。

资源准备失败后也可返回修改。K3d 重试复用本次安装的集群和凭据；主机安装开始后已有的 K3s 数据仍会触发节点检查，需要人工确认恢复，不支持自动恢复任意阶段中断的 K3s 安装。

默认配置路径为 `~/.apeiron/config.json`，部署日志与运行文件也保存在该目录下。可通过 `--config` 指定其他配置路径。
当前配置为 schemaVersion 2，保存组织、模型、应用与部署目标；旧版 LLM 配置只保留在磁盘，不回传网页。
配置文件和部署副本使用 `0600`，新建目录为 `0700`，禁止将配置保存到 Git 工作目录。
保存采用文件锁、版本检查和临时文件替换，防止旧窗口覆盖新配置。

```sh
apeiron init --no-open                       # 手动打开终端输出的地址
apeiron init --port 3210                     # 指定本地端口
apeiron init --config /external/apeiron.json  # 指定配置文件
apeiron init --help
```

页面、脚本、样式与官方 Apeiron logo、favicon 直接包含在 CLI，无 CDN、独立前端服务或 LLM 依赖。
会话使用随机访问路径，写入接口校验同源请求。部署结束后点击“完成并关闭向导”，或在终端按 Ctrl+C 停止进程与本地服务。
仅关闭网页不会停止部署。远程使用可通过 SSH 转发端口。

## 独立可执行文件

```sh
cargo build --release --locked
./target/release/apeiron init
```

构建生成当前系统架构的可执行文件，包含本地配置页面；Linux 使用 musl 静态构建，macOS 只使用系统库。使用向导无需在目标机器安装 Bun。
`onto` 转发仍需要其源码/依赖或配置好的外部 CLI。构建产物不提交到 Git。四个平台的发行构建、安装器与 GitHub/OSS 自动发布见 [发布说明](docs/releases.md)。

## Ontology 模块

默认从可执行文件目录寻找 `../ontology` 仓库，也可以指定：

```sh
export APEIRON_ONTO_ROOT=/absolute/path/to/ontology
export ONTO_MODE=remote
export ONTO_ENDPOINT=http://127.0.0.1:3101/ontology
apeiron onto status
apeiron onto schema
apeiron onto --help
```

连接与认证沿用 Ontology CLI 的配置，不内置服务地址或凭据。顶层 `apeiron verify` 检查 Ontology 入口，`apeiron status` 查看本机连接运行时；`apeiron onto status` 查看业务服务。

## 扩展模块

```sh
export APEIRON_WLK_BIN=/absolute/path/to/wlk
apeiron wlk --help
apeiron wlk <command>
```

`APEIRON_<MODULE>_BIN` 指定单个可执行文件路径，参数原样转交，不经过 shell。模块名中的 `-` 对应环境变量中的 `_`。也可用 `APEIRON_ONTO_BIN` 覆盖 onto 入口，或 `APEIRON_BUN_BIN` 指定 Bun 路径。未配置的模块返回退出码 4，不提供虚构的业务命令。

## 验证

```sh
bun install --frozen-lockfile
bun scripts/embed-assets.js --check
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bun run check
bun run test:rust
cargo build --release --locked
bun run test:ui
apeiron verify
```

开发验证使用 Rust（版本见 `rust-toolchain.toml`）与 Bun 1.3.14。`src/init/client.js`、`page.js` 是浏览器前端真源；修改后运行 `bun scripts/embed-assets.js` 更新嵌入资源。Cargo 构建直接使用已提交资源。所有 CLI 和向导服务端逻辑均在 Rust 中；Apeiron 自身不包含 TypeScript，独立的 Zygos 子模块不受此语言限制。原服务端测试已迁为 Rust 契约测试及直接启动 Rust 二进制的 JavaScript HTTP/CLI 测试。浏览器页面与构建、发行、浏览器测试脚本使用 JavaScript；资源发布和 Helmfile 验证通过 Cargo 的 build-support 示例复用 Rust 校验与配置生成，该开发工具不进入发行包。

首次浏览器验证需先运行 `bunx playwright install chromium`。`test:ui` 从临时目录启动编译后的二进制，
用隔离的部署替身验证首屏入口、八步配置、场景组合、共存预检与摘要、必选项、失败/重试、刷新恢复、部署后进入访问配置、系统授权取消/重试的页面状态、凭据读取／隐藏／复制／下载、连接检查失败与重试、完成退出、桌面/手机布局以及没有外部网络请求。授权测试使用隔离替身，不弹出真实管理员授权或修改开发机信任库。它不对真实集群执行 sync。
浏览器测试只用虚构配置，完成后删除配置和替身日志；截图保存在打印出的系统临时目录。`test:instances-ui` 用隔离的 HOME 与 docker/k3d 替身验证实例首屏与列表：登记的安装包实例、创建并由服务端分配端口、切换、诊断采集、卸载确认，桌面与手机布局。

真实 Helmfile 配置读取与编排检查（需要已有宸途工具箱镜像，不挂载 Docker socket 或 kubeconfig，不连接集群）：

```sh
bun scripts/test-helmfile.js /absolute/path/to/chentu chentu-lab
```

该检查运行原生 `print-env` / `build`，校验启用项、values 保留与依赖完整性，不执行真实集群部署。

当前尚未发布到 npm registry。`connect` / `status` / `disconnect` / `serve` 使用
`third_party/apeiron/cli` 的 Rust 连接运行时，通过 Git submodule 锁定上游提交并静态链接，
不再维护连接运行时的源码副本。SSO、IAM、Task 暂时保留本仓库现有实现；
`platform init` 已由顶层 `init` 的网页流程替代；部署由顶层 init 调用宸途原生入口，未另设 `platform install/deploy` 命令。

部署流程由 `deployment.installation.topology`（single-k3d / single-k3s / multi-k3s）
决定，生成的 Helmfile values 含相同 `topology`。不再读取 `profile` 或
`CHENTU_PROFILE`；旧配置明确报错，不自动转换。发行包 target 必须声明
`deploymentTopology: true`，与新版 Chentu 一起构建。架构来自目标节点探针，
K3d 来自运行 Docker 的本机架构；不同架构不混用安装包。原生 K3s 主机支持 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64，发行包必须用 `hostPlatform` 声明匹配的 Ubuntu 版本和架构。当前公开 rc.5 仍只提供 K3d ARM64 资源；主机兼容不代表原生 K3s 资源已经发布。多机不会自动开启 Longhorn 或三副本存储。

## 公网入口配对（新发行包）

已有 Ubuntu ECS 安装 Caddy 2.10+、OpenSSH、sudo 和 Python 3 后，在 ECS 生成配对码：

```sh
sudo apeiron platform entry pair --domain team.example.com --public-ip <公网IPv4>
```

在 Spark 等部署主机运行 `apeiron init`，选择 ECS 转发，粘贴完整配对码。配对码 10 分钟有效，只绑定一个设备；同一设备可在有效期内重试。配对后不需要提供 ECS 管理员 SSH 私钥，域名与入口地址由配对记录限定。配对码含临时认证材料，不要分享或写入日志。

部署环境与第 5 步提供状态、DNS/HTTPS 测试和确认撤销。终端也可使用：

```sh
apeiron platform connection list
apeiron platform connection status --id <id>
apeiron platform connection test --id <id>
apeiron platform connection revoke --id <id>
```

自定义配置路径需附加相同的 `--config <path>`。撤销禁用设备凭据、关闭活动转发和所属路由，保留集群数据。ECS 管理员可用 `sudo apeiron platform entry status|revoke --domain team.example.com` 确认并重试清理。首版换密钥通过撤销再配对，会中断公网访问。DNS/安全组仍由管理员配置。

需要同时发布包含 `bootstrap/public_pairing.py` 的 Chentu 安装包；当前固定的 rc.5 旧包不会被悄悄升级，缺少功能时明确拒绝。贡献者可通过显式 `APEIRON_CHENTU_ROOT` 验证对应 PR 的源码。

## Chat（Matrix）

`apeiron chat` 内置 Matrix Client-Server API 调用，不需要配置外部 Chat CLI。

```sh
export APEIRON_CHAT_SERVER=https://matrix.example.org
export APEIRON_CHAT_TOKEN_FILE=/secure/path/matrix-access-token
apeiron chat status
apeiron chat user search --query 张三
apeiron chat room create --user '@zhangsan:example.org'
apeiron chat message send --room '!room:example.org' --text '你好' --txn-id message-001
apeiron chat message list --room '!room:example.org' --limit 10
apeiron chat sync --out /tmp/chat-sync-001.json
```

凭据文件必须是当前用户拥有的普通文件、权限 0600，不接受符号链接；只读 Matrix access token，
不读取 `APEIRON_TOKEN`，不把 Apeiron SSO token 自动视为 Matrix 凭据。SSO 委托供应、自动刷新及
部署注入尚未接入。HTTP 只允许 loopback 本地测试，其他目标必须 HTTPS；请求不跟随重定向。

`--help` 列出房间加入/退出/邀请、已读、消息编辑/撤回、历史分页等命令。私聊通过私有房间邀请实现，
对方需要加入；不自动更新客户端 `m.direct` 联系人列表。查人遵循服务器目录可见性，不能枚举所有 SSO 用户。
发送、编辑、撤回必须指定 `--txn-id`，同一次操作重试沿用相同 ID 和内容；切换账号或设备后不承诺跨设备去重。
建房间没有事务幂等保证，结果不明时不能盲目重建。命令不自动重试。写操作支持 `--dry-run`。

默认输出带版本标识的 TSV 预览，正文截断至 160 字符；`--jsonl` 返回结构化结果，`--out NEW_FILE`
以 0600 权限保存完整结果且拒绝覆盖。历史默认 10 条、最多 100 条；sync 必须提供输出文件，返回
`next_cursor` 用于下一次 `--cursor`，timeline 标记 limited 时通过历史接口补齐。发送成功只表示服务器
接收，不表示对方已读。退出码：2 参数/不支持、4 不存在、5 冲突、7 鉴权/权限、8 限流、9 服务/网络。

本版本不实现端到端加密或附件。发送/编辑前检查房间状态，发现加密即拒绝；该检查不是密码学保证，
不能防止房间在检查后被并发启用加密，因此只用于明确以非加密方式运行的房间。加密消息在历史中保留
事件类型，不假装解密。Agent 使用说明见 [Chat Skill](skills/chat/SKILL.md)。

本地协议冒烟：`python3 scripts/smoke-chat.py <local-k3d-cluster> <absolute-cli-binary>`。
脚本在独立 namespace 启动 Synapse 测试服务，创建一次性用户并调用真实 CLI，结束后删除测试资源。
这是标准 Matrix API 验证，不代表已验证宸途 Tuwunel 的 SSO、生产权限或加密客户端。
### Gateway-hosted setup (development)

With the gateway portal running, generate a connection command for a reserved slug:

```sh
apeiron register --gateway https://gateway.example.com --enrollment-token <one-time-token>
```

The target machine starts the existing loopback wizard and connects outbound over WSS. Keep the process running while using setup through the gateway. The reserved slug is prefilled and enforced by the local server. The gateway never receives the wizard's loopback capability URL, and account cookies never reach the machine.

The command prints a connection-file path under `~/.apeiron/gateway/`. Reconnect with `apeiron register --connection <path>`. That file contains a private agent token and has mode 0600; do not share it. Ctrl+C disconnects and stops the wizard, including any active deployment. Automatic systemd installation is not implemented in this initial version.

Only HTTPS gateways are accepted, except `http://localhost` and `http://127.0.0.1` for local development. A localhost gateway URL works only when the CLI runs on the same machine. Production also needs gateway DNS/HTTPS and isolated setup subdomains. Application ingress provisioning remains separate from opening the deployment wizard.

### IAM CLI 与统一 SSO

IAM 命令及组织 API 客户端由 [apeiron-iam](https://github.com/TheApeironLab/apeiron-iam) 的
`cli/` Rust crate 维护，通过 `third_party/apeiron-iam` Git submodule 固定源码提交并静态链接。
本仓库的 `runtime/src/apps/iam.rs` 仅接入统一 SSO、命令执行和通用输出；最终仍为单个
`apeiron` 二进制，无 Node.js 或外部 IAM CLI 运行依赖。

开发 checkout 需先执行 `git submodule update --init --recursive`。更新 IAM 时先提交上游，
再更新固定 gitlink 和必要的 Cargo.lock；不要在构建中跟随远端分支。IAM 自身契约测试位于
`third_party/apeiron-iam/cli/tests`，本仓库保留 SSO 到 IAM 命令的集成测试。
CI 的 `ZYGOS_READ_TOKEN` 还需具备 `TheApeironLab/apeiron-iam` 的 Contents: read 权限；
该变更不自动扩大凭据权限。

`apeiron sso login --server <APEIRON_URL>` 后，`apeiron iam` 直接调用该平台的 `/api/organization`。IAM 网页和 CLI 共用同一组织 API 与 `apeiron-app` audience，不创建 IAM 换票 broker，不把 Keycloak Admin API 的服务账号凭据交给 CLI。服务端每次请求读取当前账号状态、组织归属和管理角色；`iam-user` 入口资格不授予成员管理权限。

```bash
apeiron iam organization get --json
apeiron iam member list --offset 0 --json
apeiron iam app list --json
apeiron iam operation list --json
apeiron iam member update <SUBJECT> --file access.json --idempotency-key <UUID> --json
```

`member create`、`member update`、`member reset-password` 的 JSON 文件沿用组织 API 契约，`--file -` 可从 stdin 读取。密码不通过命令参数传递。所有写命令必须指定 UUID 幂等键；未知结果先查询操作记录，再使用原幂等键与原输入重试，不自动生成新操作。开户的邮箱核验声明仍须基于真实组织核验。CLI 不接受任意 API URL 或租户参数，组织由 SSO 身份决定。

Zygos 可在安装向导中选择，需使用包含其 API、Web、Worker、EvalScope 四个镜像的新版 Chentu 安装包。旧版在线包不包含 Zygos，因此当前默认应用选择保持兼容；使用新版包时显式勾选 Zygos。GPU 节点与模型权重单独配置。

## 单应用安装、更新与卸载

对已完成 `apeiron init` 的实例操作，显式指定实例，避免操作到其它集群：

```sh
apeiron app list --instance my-team
apeiron app install zygos --instance my-team --bundle-dir /absolute/chentu-package --dry-run
apeiron app install zygos --instance my-team --bundle-dir /absolute/chentu-package
apeiron app update zygos --instance my-team --bundle-dir /absolute/next-package
apeiron app update git ci --instance my-team --bundle-dir /absolute/next-package --dry-run
apeiron app uninstall zygos --instance my-team --dry-run
apeiron app uninstall zygos --instance my-team --yes
```

也可使用 `--config /absolute/config.json`。程序仍是 Rust 单文件，运行不需要 Bun/Node。
安装包须包含新版 Chentu 的应用生命周期入口，已有集群与资源身份会先被核验。
`install` 追加应用，依赖应用和共享服务须已安装；`update` 同步当前或指定包中固定的版本，
不会自动选择公网 latest。应用包不得隐式改变未选应用或共享服务；关联应用须显式联合选择，
共享服务变更使用独立维护流程。
`app update` 可明确选择多个已安装应用，使用同一包、安装锁与 `appOperation` 检查点。
联合范围由新包的主 Helmfile `application` 标签展开并在预览时与请求核对，原生 needs 决定执行顺序；
旧包或范围不一致在修改前拒绝。未选应用和共享服务的制品兼容守卫继续生效，
不因联合选择而自动更新共享依赖。联合更新可能部分成功，失败后按同一选择与包核查重试。
新操作在既有 `appOperation` 中保存稳定 operation ID，重试保留该 ID；新 Chentu
使用它保留迁移 Job。既有迁移尝试要求核对 Job 和数据库账本，不会通过删除 Hook 重放。
支持操作 API 的新 Chentu 检查点可用 `apeiron operation status --instance my-team --json`
及 `apeiron operation events --instance my-team --after 12 --json` 只读观察；也支持显式 `--config`。
两者复用同一检查点和包内入口，不查询集群、不取安装锁、不重试或接管操作。
旧检查点明确返回 `UNSUPPORTED_OPERATION_CHECKPOINT`。`verifying`、应用验收和配置提交
分别显示；向导也通过同一进程接口展示这些事实和事件，刷新不会重试安装。
持久记录拒绝回落到旧 app 同步重试入口，业务验收通过但配置未提交时继续显示待提交。
该观察入口不表示 app 命令已经交给后台执行器，实际提交和资格核验接线仍在开发中。
更新及卸载后重装保留运维配置，只推进所选应用的镜像与 Chart。安装/卸载另同步 Apeiron 的应用目录。
新 Chentu 的 Nexus-first 应用路径复用现有 Nexus，不同步引导依赖、切换注册表或重启节点；
旧包继续按其自身驱动契约执行，包兼容预检不能省略。

`uninstall` 停止并移除应用服务，保留 PVC、凭据、共享数据库和对象存储，不提供清空数据开关。
基础应用和仍被依赖的应用不能卸载。老版本若未在 Helm 记录中声明数据保留策略，须先执行
`app update`；存在删除 hook 的 release 会拒绝通用卸载。Zygos 卸载前检查活动评测，维护前须结束 Worker 任务。

`--dry-run` 校验资源、目标集群和卸载策略，不修改集群或实例配置，会生成本地预览日志。
应用选择在操作成功后才保存。失败可能已部分改变集群，不自动回滚数据库；使用完全相同的命令
和包重试。检查点会拒绝变动资源、错误集群及并发部署。`app list` 展示配置中的选择，不代替服务健康检查。

## Zygos 内置子命令

`apeiron zygos` 静态链接 Zygos Rust CLI，单个 apeiron 文件即可使用；
不需要外部 zygos 可执行文件，也不读取 `APEIRON_ZYGOS_BIN`。

```sh
apeiron zygos --help
export ZYGOS_API_BASE_URL=http://127.0.0.1:8000
apeiron zygos model list
```

连接、认证、JSON 输出、字段选择和退出码沿用 Zygos CLI。API 服务仍需单独部署。
本机连接的上游是 `third_party/apeiron`，接收端上报的软件版本来自上游 `VERSION`；
宿主 `apeiron --version` 仍报告本 CLI 的发行版本。更新时先在 Apeiron 上游验证并合并，
再更新 gitlink 与 Cargo.lock；不要在构建中追踪浮动分支或复制运行时代码。
宿主保留真实可执行文件的生命周期集成测试，CI 另外运行 vendor 内的 Rust 检查与测试。
CI 使用的 `ZYGOS_READ_TOKEN` 需要同时能读取本仓库、Zygos 与 Apeiron；名称沿用现有配置。

源码通过 `third_party/zygos` Git submodule 锁定确切提交，Cargo 直接引用其中的
`apps/cli`，不复制业务 CLI 源码。开发与更新方式见 [Zygos 子模块](docs/zygos-submodule.md)。
克隆开发仓库后先运行 `git submodule update --init --recursive`，最终用户不需要此步骤。
