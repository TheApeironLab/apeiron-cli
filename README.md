# Apeiron CLI

统一入口：`apeiron <模块> <命令>`。`apeiron init` 提供内置本地部署向导，`apeiron onto` 接入现有 Ontology CLI。TypeScript strict + Bun 1.3.14+；onto 模块需要已安装依赖的 ontology 仓库。

## 安装发行版

macOS / Linux，ARM64 或 x64：

```sh
curl -fsSL https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com/apeiron-cli/install.sh | sh
apeiron init
```

安装器自动选择平台并校验 SHA-256，优先原位更新已能找到的 `apeiron`，否则选择当前 PATH 中的常用 bin 目录。用户目录可直接写入；系统目录不可写时会请求 sudo 授权。安装后当前终端即可直接运行 `apeiron`，无需手动修改 PATH。`apeiron init` 自动打开默认浏览器。重新运行安装命令可升级 CLI，不会重新部署平台或修改配置。独立文件也可从 GitHub Releases（需要仓库权限）或公开 OSS 下载；离线安装和固定版本见 [发布与安装说明](docs/releases.md)。

CLI 可运行的平台与 Chentu 安装包支持的部署目标分别校验；当前预览包的全新部署目标为 **K3d ARM64**。Docker 等部署依赖仍需准备。

## 源码开发安装

```sh
git clone https://github.com/TheApeironLab/apeiron-cli.git
cd apeiron-cli
bun install --frozen-lockfile
bun link
apeiron --help
```

## 浏览器部署向导

```sh
apeiron init
```

命令监听 `127.0.0.1` 的空闲端口，自动打开网页。setup 分为六步：

1. **部署环境**：依次展示系统配置、网络测试、部署选项；选择在线／离线、单机 K3s／单机 K3d／多机 K3s。多机时展开节点连接与 SSH 检测。
2. **组织**：填写 Slug name，自动生成 `<slug>.apeironlab.internal`；可展开使用自定义域名。这里只确定名称，不显示访问配置或要求解析通过。
3. **应用选择**：确认要启用的应用，点击“开始部署”。
4. **部署**：完整准备并校验资源，创建集群，再运行宸途 `Helmfile sync`。显示进度、结果和日志入口。
5. **配置访问**：部署成功后自动进入。在有桌面会话的 Mac 上可点击“一键配置本机访问”；其他电脑使用折叠的手动指引。可以返回查看部署记录，不会再次部署。
6. **测试**：从访问配置点击“下一步：测试”。展示初始管理员用户名、默认隐藏的密码，提供显示／复制／下载，以及 Apeiron／Ops 登录入口。可以测试三处核心入口的解析和 HTTPS。

模型连接页面已移除。应用的版本与默认声明由兼容的宸途发行包提供，向导不编造模型或私有镜像配置。

| 选择规则 | 应用 |
| --- | --- |
| 必选，不可取消 | Nexus、Vasi、Limani、Apeiron（含 Ops） |
| 默认选中，可取消 | Task、Corpus、Chat（团队聊天）、Files、邮件 |
| 默认不选 | Gateway、Filer、代码仓库、GPUStack、Langfuse、Grafana |

默认选中 9 项，其中 4 项必选。页面按上表分三组、按表内顺序展示。重新打开保留可选应用选择；旧配置中缺少的必选应用会自动勾选，提交时才写入。可选应用仍受发行包声明的部署依赖约束。

### 自动环境探测

页面依次展示两个独立区域：第一块“系统配置”显示操作系统、内核、架构、逻辑核心、内存和 CPU；第二块“网络测试”用 Microsoft、GitHub、Google、下载源四张卡片显示连接状态与耗时。完整域名、HTTP 状态码和网络说明放在默认折叠的“检测详情”中，异常结果也不会自动展开。图标随 CLI 内嵌，无外部图片请求。

第一步加载配置后，由 CLI 在本机检测操作系统（Linux 发行版/macOS/Windows）、硬件架构（AMD64/ARM64 等）、逻辑核心数与内存。macOS 上区分 ARM 硬件与 Rosetta 下运行的 x64 CLI。Linux 的运行时没有提供 CPU 名称时，回退到 `lscpu --json`，保留异构 CPU 的所有核心型号；仍无法识别时显示“CPU 型号未提供”。信息指运行 CLI 的主机，远程 K3s 节点不在此探测范围。

在线模式向 `www.microsoft.com`、`www.google.com`、`api.github.com`、`release-assets.githubusercontent.com` 发起并行 HTTPS HEAD 请求，每项超时 4 秒，不携带配置或访问令牌，不跟随重定向。Google 单独展示，用于检查该站点的海外访问情况，不代表所有海外网站都可用。结果区分已连接、已响应但 HTTP 错误、DNS/TLS 失败和超时；下载域名根路径返回 404 时，以提示颜色注明“根路径没有资源，尚未验证具体安装包能否下载”，不能据此判断安装包存在或拥有下载权限。结果仅代表固定探测点的连通性，不自动切换部署模式或阻止继续填写。

已保存的离线配置加载后不发起外网检测；页面切换到离线时取消正在执行的探测，仅显示本机信息。30 秒内复用最近结果，点击“重新检测”立即刷新。刷新检测不保存配置，也不连接 Kubernetes。

### 全新安装

第三块“部署选项”直接在页面内选择，不使用 CLI flags，也不提供“使用已有集群”。

| 方式 | 目标与限制 | 执行入口 |
| --- | --- | --- |
| 单机 K3s | 当前主机；现有宸途安装器支持 Ubuntu 22.04 / AMD64、root 或免密 sudo | 自动生成 local inventory → `bootstrap/hosts.yaml` → Helmfile |
| 单机 K3d | 本机 Docker；开发测试，使用发行包校验后的工具箱与镜像 | `tests/lab/helmfile.sh prepare` → `sync` |
| 多机 K3s | SSH 连接的 Ubuntu 22.04 / AMD64 节点；管理机可为 macOS | 校验并分发资源 → 自动生成 inventory → `bootstrap/hosts.yaml` → Helmfile |

普通多机至少 2 台，1 个控制节点加工作节点。控制平面高可用需至少 3 个、且为奇数个控制节点；不把“至少 3 台”强加给普通多机。SeaweedFS 使用 1 或 3 个存储节点；两节点部署使用 1 个 SeaweedFS 节点，Longhorn 副本数不超过节点数。这不等于所有应用都高可用。

节点区可导入 `~/.ssh/config` 的显式 Host 别名，或每行输入一个地址。点击“自动检测节点”后通过 SSH 读取系统、架构、CPU、内存、根磁盘可用空间、内网 IPv4、sudo 权限与已有 K3s 数据，并生成可调整的名称／IP／角色。修改连接参数后检测结果失效。使用已有 SSH 配置或 Agent，可另填私钥路径；不会读取私钥内容到网页。严格检查 known_hosts，首次连接需先用 SSH 确认指纹。最多 32 节点，每批并行 4 台。

安装前重新检测节点；多机还通过 Ansible 检查节点间 SSH 端口连通。K3s 端口尚未监听，不据此宣称防火墙已全面通过，实际加入由宸途检查。已有 K3s 数据会阻止全新安装，不自动清除或接管。域名 DNS、入口 VIP 和数据盘准备仍需完成；此版本不配置 DNS／VIP，不提供安装中断后的 K3s 接管或自动回滚。

部署环境步骤为 K3s 建议单机主机的网卡 IP 或普通多机的控制节点 IP；HA 模式需显式填写已配置的稳定入口 IP，向导不创建负载均衡或 VIP。内网 DNS 需配置 `*.<domain> A <entryIp>`；根域名记录也会显示，但应用入口为 `apeiron.<domain>` 等子域名。工作站和节点需使用该 DNS。部署完成页的解析检查由用户点击触发，从 CLI 主机的系统解析器发起（包含 hosts／macOS scoped resolver），并行检查 Apeiron、IAM 和随机子域名，要求所有返回地址与入口 IP 一致，单项最多 4 秒。重新部署会作废旧结果，完成页使用本次成功部署的域名和入口 IP。K3s 安装前复查管理机的泛解析，Ansible 在安装节点前核验每个节点的 Apeiron／IAM 解析。此检查不代表浏览器、Pod、TLS 或应用健康检查已经通过。

本机 K3d 默认使用 HTTP `54320`、HTTPS `54321`，可在部署环境中修改；应用 URL、登录回调与访问测试使用同一组端口，K3s 仍使用 80/443。点击开始部署时先检查端口：同一配置和域名持有的运行中集群按原端口重新部署，保留卷和凭据；其他服务占用、端口不匹配或 Docker 未启动时停留在配置页，不保存配置、下载资源或启动部署。创建集群前再次检查，避免检查后端口被抢占；不自动结束其他服务，也不接管无归属记录的集群。部署成功后的“配置访问”提供 Apeiron／IAM 的 hosts 示例和解析检测，本机模式只检测这两个名称，不要求 hosts 支持泛解析。部署成功后从宸途自有的 `chentu.lab.ingresshosts` 清单工具生成完整 hosts 文件。CLI 不在部署期间修改工作站的 hosts 或 DNS。部署后的 Mac 一键配置须由用户点击，合并本次域名的受管 hosts 区块，保留其他条目并备份原文件；检测到同名域名指向不同 IP 或不完整的区块时会停止，不覆盖冲突。手动配置仍需合并条目，不能覆盖原 hosts 文件。

部署成功后提供平台链接、公开 CA 证书下载、SHA-256 指纹、有效期，以及 macOS／Windows／Ubuntu 的信任指引。CA 由宸途签发，CLI 只读取本次部署导出的证书，检查有效期和 CA 属性，拒绝私钥、混合 PEM 与符号链接；文件接口仅开放固定的本次产物，不接受用户文件路径。证书或 hosts 提取失败会单独提示，不伪造下载文件，也不会把已成功的 Helmfile 同步改成失败。CA 私钥备份、根 CA 轮换和全节点信任分发仍遵循宸途现有流程，当前 CLI 未增加自动轮换。

Mac 一键配置使用 [AppleScript 系统管理员授权](https://developer.apple.com/library/archive/documentation/AppleScript/Conceptual/AppleScriptLangGuide/reference/ASLR_cmds.html)：密码只输入 macOS 弹窗，不发送给网页或 CLI。将**本次成功部署**的公开 CA 加入系统钥匙串，并仅设置 SSL 信任用途；在 `/private/etc/apeiron-access.*/hosts.before` 留下 hosts 备份。安装命令嵌入经过校验的证书及 hosts，不接受浏览器传入的路径、命令或证书；系统授权后再次核对 hosts 内容，避免覆盖期间发生的其他修改。取消授权可以重试，重复安装只替换当前域名的受管区块。操作期间禁止关闭向导、修改配置或重启部署；刷新网页只读取状态。

完成后从 CLI 主机检查 Apeiron／IAM 的系统解析和 HTTPS（使用系统信任、不跳过证书验证、不走环境代理），不把访问失败记成部署失败。浏览器独立信任库和代理仍可能需要单独设置；应让 `*.<平台域名>` 直连。页面始终标注实际被配置的 CLI 主机；非 macOS、SSH 或无桌面会话不提供一键按钮，可下载公开 CA 与 hosts 在浏览器所在电脑手动配置。不会把远程机器的系统配置误称为当前浏览器电脑已配置。

测试页只在成功部署后通过同源 POST 读取本次生成的 kubeconfig 所指向的 `keycloak/keycloak-bootstrap` Secret，仅返回 `username`、`password`。Docker 模式复用已安装工具箱镜像，以只读 kubeconfig 和集群网络查询，无 Docker socket、不拉取镜像。口令不进入进程参数、配置快照、状态接口或部署日志，也不自动写入本机凭据文件；用户点击下载时才生成文件。离开测试页／关闭向导清除页面密码，刷新后重新读取并恢复隐藏状态。初始 Secret 不会跟踪用户之后在 IAM 中修改的密码，因此页面明确说明其仅为首次安装凭据。

连接测试检查 Apeiron、Ops、IAM：从 CLI 主机解析地址，必须全部匹配部署入口，再使用系统证书信任发起 HTTPS HEAD。固定入口 IP、禁用环境代理、不跟随跳转、不会发送账号密码；HTTP 2xx／3xx 视为入口可访问，404 等错误或 TLS 失败不会通过。它不宣称用户已登录或应用功能全部正常；用户用显示的凭据打开应用完成登录测试。


离线模式可选择 CLI 主机上的目录；macOS 桌面可打开目录选择器，远程／无桌面环境直接填写绝对路径。离线不下载资源，缺少所选应用或依赖的文件、大小或 SHA-256 不符即停止。在线模式只下载缺失或损坏的所需文件，必须收到完整 HTTP 200 响应并通过大小与 SHA-256 校验；404、截断响应与校验失败不能放行集群创建。公共网站卡片不充当资源检查门槛。

**发行状态：CLI 固定使用兼容的 Chentu `0.1.0-rc.5` 预览包。** 包内提供 `deploymentTopology`、`clusterOidc`、自定义入口端口和 `setup/install.json`，按所选应用下载并校验资源。目前提供 `k3d-arm64` 目标，共 50 个资源文件，覆盖默认 9 个应用及可选 Gateway、Filer、代码仓库、Langfuse、Grafana；GPUStack 尚无对应资源，选择后会在修改集群前明确拒绝。

源码基础提交为 `f109f8567b9df0cf1eeb1b6d6cbd7092be5f99c3`，具体修复以包内 `chentu-package.json.sourceOverlay` 的逐文件摘要为准。安装包 SHA-256：`f45a9a370a15e01697ba82d8ad1101853c040a3b1bbf1c0e1f557164d4706021`。来源、验证与限制见 [release.json](https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com/chentu/releases/0.1.0-rc.5/release.json)，消费契约见 [docs/install-package.md](docs/install-package.md)。已验证本机 ARM64 K3d 部署与 Vasi 集群 SSO 权限；原生 Ubuntu、AMD64、多节点和断网安装尚需分别验收。Apeiron 的真实 LLM 需部署后配置，开发模型桩不代表真实对话已可用。

环境 values、inventory、kubeconfig 由流程生成，不向用户索取。页面不再提供源码路径或开发选项：在线使用匹配版本的发行包，离线使用所选目录中的 `chentu/` 部署程序与应用资源。旧配置中的源码覆盖不会沿用到全新安装。仅 CLI 开发联调可显式设置进程变量 `APEIRON_CHENTU_ROOT`，该变量不写入安装配置，离线模式也不会使用它。

### 配置与进度

每次部署在配置目录的 `deployments/run-*/` 生成 `environment.yaml`、`inventory.yaml`（K3s）与 `install.log`。
生成器以发行包声明为基础写入组织、域名、节点与存储规模，以及应用 `releases.<id>.enabled`。Helmfile 仍负责 release 默认值与 topology 预设合并、依赖校验、安装顺序和 hooks；资源清单仅负责文件选择与下载。

Limani 对应 `ontology`，Chat 对应 `matrix`，邮件对应 `stalwart`，代码仓库对应 `git`。Grafana 选项控制宸途 observability 组件的 `kps`、`loki`、`promtail` 三个 release；其余 ID 为小写应用名。Apeiron 包含 Ops，无需单独选择。
取消选择只将相应 release 排除于本次同步，不卸载已有应用；平台访问权限仍由平台管理。

网页展示部署状态、耗时、退出码、已识别的同步进度与日志路径。准备阶段的进度与错误也写入日志，失败时不会只留下空文件。“在网页打开日志”以纯文本展示本次运行的完整日志，刷新日志页可查看最新内容；“下载日志”保存同一文件。日志仍保存在 CLI 主机，使用 `0600` 权限；访问须经过本次向导的随机令牌和来源检查，接口不接受文件路径，拒绝符号链接。每次重试生成独立日志，页面链接随当前运行更新。完整输出可能含敏感诊断，不写入常规状态响应或终端；保管好向导链接及下载的日志。
重复启动会被阻止，刷新页面会恢复当前进度。同一源环境文件的多个向导共享部署锁。
第 4 步提供“停止部署”。停止资源下载或本机部署进程，并清理本次运行的临时工具箱；不会删除集群、卷或回滚已完成的变更。已提交给 Kubernetes 的任务可能继续运行。停止期间禁用修改和重新部署；清理失败会保留部署锁，检查 Docker／目录权限后可“重试停止”。关闭、刷新页面不会发起停止。

停止或失败后，可点击“重新部署”，使用上次保存的配置重新检查资源并运行部署，每次生成独立日志。这不是断点续跑，hooks 可能重复执行。配置在其他窗口或进程中变化后，必须返回修改确认。每次 sync 前通过当前集群的 Helm 元数据检查 pending-install／pending-upgrade／pending-rollback；检测到未完成操作或读取失败时，阻止本次 sync，提示涉及的 release，不自动删除 Helm 记录或回滚。

资源准备失败后也可返回修改。K3d 重试复用本次安装的集群和凭据；主机安装开始后已有的 K3s 数据仍会触发节点检查，需要人工确认恢复，不支持自动恢复任意阶段中断的 K3s 安装。

默认配置路径为 `$XDG_CONFIG_HOME/apeiron/config.json`，未设置时为 `~/.config/apeiron/config.json`。
当前配置为 schemaVersion 2，保存组织、应用与部署目标；旧版 LLM 配置只保留在磁盘，不回传网页。
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
bun run build
./dist/apeiron init
```

构建生成当前系统架构的可执行文件，包含本地配置页面和 Bun 运行时，使用向导无需在目标机器安装 Bun。
`onto` 转发仍需要其源码/依赖或配置好的外部 CLI。构建产物不提交到 Git。四个平台的发行构建、安装器与 GitHub/OSS 自动发布见 [发布说明](docs/releases.md)。

## Ontology 模块

默认寻找相邻的 `../ontology` 仓库，也可以指定：

```sh
export APEIRON_ONTO_ROOT=/absolute/path/to/ontology
export ONTO_MODE=remote
export ONTO_ENDPOINT=http://127.0.0.1:3101/ontology
apeiron onto status
apeiron onto schema
apeiron onto --help
```

连接与认证沿用 Ontology CLI 的配置，不内置服务地址或凭据。顶层 `apeiron status` / `apeiron verify` 检查本地入口；`apeiron onto status` 查看业务服务。

## 扩展模块

```sh
export APEIRON_WLK_BIN=/absolute/path/to/wlk
apeiron wlk --help
apeiron wlk <command>
```

`APEIRON_<MODULE>_BIN` 指定单个可执行文件路径，参数原样转交，不经过 shell。模块名中的 `-` 对应环境变量中的 `_`。也可用 `APEIRON_ONTO_BIN` 覆盖 onto 入口，或 `APEIRON_BUN_BIN` 指定 Bun 路径。未配置的模块返回退出码 4，不提供虚构的业务命令。

## 验证

```sh
bun run typecheck
bun test
bun run build
bun run test:ui
apeiron verify
```

首次浏览器验证需先运行 `bunx playwright install chromium`。`test:ui` 从临时目录启动编译后的二进制，
用隔离的部署替身验证六步配置、必选项、失败/重试、刷新恢复、部署后进入访问配置、系统授权取消/重试的页面状态、凭据读取／隐藏／复制／下载、连接检查失败与重试、完成退出、桌面/手机布局以及没有外部网络请求。授权测试使用隔离替身，不弹出真实管理员授权或修改开发机信任库。它不对真实集群执行 sync。
浏览器测试只用虚构配置，完成后删除配置和替身日志；截图保存在打印出的系统临时目录。

真实 Helmfile 配置读取与编排检查（需要已有宸途工具箱镜像，不挂载 Docker socket 或 kubeconfig，不连接集群）：

```sh
bun scripts/test-helmfile.ts /absolute/path/to/chentu chentu-lab
```

该检查运行原生 `print-env` / `build`，校验启用项、values 保留与依赖完整性，不执行真实集群部署。

当前尚未发布到 npm registry。顶层 `status` / `verify` 仍沿用早期 Ontology 入口检查含义；
`platform init` 已由顶层 `init` 的网页流程替代；部署由顶层 init 调用宸途原生入口，未另设 `platform install/deploy` 命令。

部署流程由 `deployment.installation.topology`（single-k3d / single-k3s / multi-k3s）
决定，生成的 Helmfile values 含相同 `topology`。不再读取 `profile` 或
`CHENTU_PROFILE`；旧配置明确报错，不自动转换。发行包 target 必须声明
`deploymentTopology: true`，与新版 Chentu 一起构建。架构来自目标节点探针，
K3d 来自运行 Docker 的本机架构；不同架构不混用安装包。原生 K3s 当前仍需
Ubuntu 22.04 AMD64，多机不会自动开启 Longhorn 或三副本存储。
