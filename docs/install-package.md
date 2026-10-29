# 宸途全新安装资源契约

CLI 固定下载 OSS `0.1.0-rc.5`，提供新版 `deploymentTopology` / `clusterOidc` 契约。目前包含 `k3d-arm64` 目标及 14 个应用（不含 GPUStack）的 50 个资源文件；其他架构、原生 K3s 和 GPUStack 会在变更集群前拒绝。源码基础为 `f109f8567b9df0cf1eeb1b6d6cbd7092be5f99c3`，修复以 `chentu-package.json.sourceOverlay` 的逐文件摘要记录。具体发布与验收范围见 README。

发行包内 `setup/install.json` 使用 `schemaVersion: 1`，包含：

- `targets`：`k3s-amd64`、`k3d-amd64`、`k3d-arm64` 等已验证目标；每个目标指定资源组件 `base`、可部署的 `environment` 和 K3d 所需的 `toolboxImage`（固定 SHA-256 digest），或 `toolboxArchive` + `toolboxImageId`（已校验归档及实际 Docker 镜像 ID）；`dockerArchives` 记录 K3d 系统归档和各镜像名称／ID。未提供的目标不能安装。
- `components`：按应用 ID 对应资源组件，字段 `requires` 是资源依赖，`files` 是文件清单中的路径，`values` 是合入发行包基础环境的固定配置。每个用户选择的应用必须有记录。`images` 将归档映射到内部 registry 引用及 manifest digest；同一引用不允许冲突版本。此接口不排序 Helm releases、不安装 Charts，也不代替 Helmfile 的依赖真相源；发行端应从解析后的 Helmfile 与 artifacts 生成并验证它。
- `files`：每项具有安全相对 `path`、正整数 `size`、64 位十六进制 `sha256` 和可选 HTTPS `url`。不接受目录穿越、符号链接或重复路径。下载 URL 是实际制品地址／代理地址；不会检查注册表根路径后就宣称制品存在。预期的字节来自固定发布版本，不能运行时信任一个刚下载文件的自身散列。

每个 target 必须声明 `deploymentTopology: true`，其环境与部署入口使用 `topology`，不接受旧 `profile`。Vasi 必选，因此目标还必须声明 `clusterOidc: true`，资源依赖包含 `cluster-access`，部署程序包含 `cli/src/chentu/cluster_oidc.py` 和 `bootstrap/oidc.yaml`。这些标记代表发行包实际实现的能力，不能仅修改清单跳过检查。

环境字符串可使用 `__DOMAIN__`、`__REGISTRY__`、`__NODE__`、`__WORK__`、`__BUNDLE__`、`__ARCH__`。这些值在 CLI 生成外部 YAML 时替换，覆盖所有层级的对象键、字符串值及数组成员；例如 `imagePins` 的 `__REGISTRY__/busybox:1.36` 键必须与实际注册表镜像名称一致。替换产生重复对象键时拒绝安装，不能静默覆盖镜像 digest（回归：#28）。具体应用镜像、模型默认声明、skills bundle 等必须由发行方提供可部署值；占位版本或 `unset` 应被宸途检查器拒绝。模型配置可以通过发行包显式声明延后，这不代表模型对话已经可用。

K3s 目标还必须声明 `hostPlatform: {os: "ubuntu", version: "22.04" | "24.04", architecture: "amd64" | "arm64"}`，当前支持的原生组合为 Ubuntu 22.04 / AMD64 和 Ubuntu 24.04 / ARM64，必须与每一台节点的实际系统及架构匹配。缺失或不匹配时在创建集群之前拒绝。发布工具从已校验的 `bundle.yaml` 的 `ubuntu`（jammy / noble）和 `arch` 生成此字段，并将该文件列入资源校验清单。不能只修改架构标签后复用另一系统版本的 Deb 包。

K3s 资源需包含兼容的系统包、K3s 二进制／安装脚本／airgap 镜像／审计策略、CLI wheels、工具、Charts、应用镜像 tar 与安装时生成的 `images/install-manifest.json`（节点按其中的 reference 与 digest 重标记）。镜像 manifest／架构／digest 的一致性应在发布时完成验证，不能只把任意文件列入清单。必要的上游镜像包括 docker.io、ghcr.io、quay.io，以及启用相应组件时的 codeberg.org、registry.k8s.io、nvcr.io；自研应用的私有镜像同样必须显式纳入。安装时资源必须全部在本地、匹配大小和 SHA-256 后才能继续。

在线模式接受 HTTPS 重定向（最多 5 次、不携带凭据），完整下载到临时文件，校验后原子替换缓存。若下载需要身份，发行方需提供可直接访问的制品地址；当前该资源接口不会自动继承 GitHub、Docker 或任意域名的凭据。宸途部署程序使用独立的公开 OSS 下载器，仅允许固定发布目录，并由 CLI 内置 SHA-256 与源码提交校验。

离线目录包含 `chentu/` 部署程序及其 `setup/install.json`，资源位于目录下的清单相对路径。目录选择授权使用其中的部署程序，应来自受信任的发布方。离线资源准备不调用网络下载，支持选择 K3d 并使用本地完整安装包。选择的文件被复制到独立工作目录，再次校验并生成本次专用 `SHA256SUMS`，因此清单不得自行列入 `SHA256SUMS`。

应用的启用状态必须与资源组件闭包一致：用户选择的应用及其传递依赖才会启用，不能沿用旧包基础环境中默认开启的可选应用。组件的具体 values 和基础设施配置保留；Grafana、Browser、CI 等应用使用目录声明的 Helm release 别名。此规则兼容默认开启全栈的旧目录，也兼容默认关闭、由组件 values 开启的新目录。

发布流水线在 Linux ARM64 构建中运行 `bun scripts/test-install-plan.js`：下载并校验运行时固定的公开 Chentu 源码包与工具箱，验证“仅四项必选”、Task、Langfuse 和 Grafana 四种选择。测试调用真实 Helmfile 和包内 `check.py`，工具箱没有网络、Docker socket 或 kubeconfig。它验证配置和依赖，不代表真实集群部署成功。本地可传入已校验的包与工具箱：`bun scripts/test-install-plan.js /path/to/chentu sha256:<toolbox-image-id>`。

安装顺序：

1. 校验应用资源闭包与目标平台，检查管理机工具和目标节点；K3s 还检查管理机到平台入口的泛解析。
2. 下载／检查全部所需资源；离线缺失即报错。
3. 生成 Helmfile 环境并调用宸途 `check.py`，在节点安装前发现无效应用配置。
4. K3d 检查所选本机端口（默认 54320／54321）、导入已校验工具箱／系统镜像并核验实际镜像 ID，再 `prepare`；K3s 生成受保护 inventory，Ansible 检查节点 DNS 和互联、分发与校验资源，再执行原生 `bootstrap/hosts.yaml`。
5. 使用生成的 kubeconfig 调用现有 Helmfile `sync`，记录真实退出码。
6. 同步成功后运行集群 SSO 初始化，验证 OIDC 认证及管理员／只读权限；失败时安装仍记为失败，允许停止与重试。
7. 成功后读取 `work/state/chentu-ca.crt` 公开 CA，生成用户可下载的证书与信任指引；使用发行包内标准库工具 `cli/src/chentu/lab/ingresshosts.py` 生成完整 hosts 清单。缺少产物单独报告，文件接口不会读取私钥。K3d 的 `work: /work/state` 映射到 CLI 的工作目录，原生部署同样写入该目录。入口 IP 仅用于 DNS 指引与检查，不能据此认为 VIP 已创建。

当前安装脚本仍有发布前验收项：原生 Ubuntu、实际跨机分发／防火墙、GPU 额外主机配置、断网场景、安装中断恢复。测试中的替身成功不能作为这些场景已通过的证据。

本机 K3d 默认使用 HTTP 54320 / HTTPS 54321；目标需声明 `publicPorts: true` 才支持非标准端口。点击开始部署时先检查 Docker、归属与端口：匹配本次安装的集群直接重新部署，其他服务占用时在配置页阻止启动。

本机 K3d 的工作目录按配置路径与域名固定。重试仅复用带有本安装归属记录且 Docker 集群标签匹配的集群；未持有该归属记录的同名集群会被拒绝，不删除卷。镜像推送前检查已存在的 manifest digest，一致时复用，写入后再比对 digest。

K3d 目标的 `k3sAirgap` 指向资源闭包中已校验的基础镜像包，创建节点时挂载到 K3s 的自动导入目录。CLI 为 rc.5 的 K3s `v1.36.3+k3s1` / ARM64 补充固定大小、SHA-256 与 OSS 下载地址；版本不匹配即拒绝，不复用另一版本镜像。基础镜像与应用资源一起下载校验，Traefik 真正 Ready 后才进入 Helmfile sync，等待上限 600 秒。准备期间每 15 秒显示等待时长、基础 Pod 状态变化及拉取事件，并写入安装日志。该改动只解决 K3s 基础服务的离线启动，不代表完整应用部署已通过断网验收；已开放离线 K3d 供实际试跑。

## OSS 资源存储与迁移

新资源存放在同一公开 OSS bucket 的 `apeiron/blobs/sha256/<完整 SHA-256>`，对象名不附加文件名、版本号或平台。`setup/install.json` 继续作为安装资源的唯一清单：`path` 保留离线包内文件名，`size` 与 `sha256` 固定预期字节，`url` 指向该内容对象。平台、安装目标、组件依赖仍由原有 targets/components 描述，不能从 blob 路径推断。相同字节只上传一次，内容变化必须使用新摘要地址。

发布者在包含 `chentu/setup/install.json` 的完整本地 bundle 上运行：

```sh
bun scripts/publish-resources.js /absolute/bundle /absolute/output --plan
bun scripts/publish-resources.js /absolute/bundle /absolute/output --publish
```

输出目录必须位于输入 bundle 外。两种模式都先校验全部资源的大小、摘要和路径；`--plan` 只生成本地上传计划和 `install.plan.json`。`--publish` 使用已有 Aliyun CLI 身份上传缺失对象，随后通过公开 HTTPS 地址流式下载、校验全部字节；已有内容不匹配即失败。只有全部对象通过才输出 `install.json`。中断可重跑，已验证对象可复用；不删除旧资源。

将输出清单纳入**新的**宸途发行包，并重新生成包的来源摘要、归档摘要和发行元数据。不能覆盖已发布包或原位修改它的清单；旧 rc.2 / rc.5 URL 继续保留，原有 CLI 和离线包无需迁移。新地址仍使用 schemaVersion 1，在线下载与离线校验无需另一套解析器。

此次先迁移资源存储。统一发行组合 manifest、stable/preview channel 和按摘要的本地共享缓存是后续阶段；当前 CLI 固定的发行包不会因上传资源而自动切换。资源公开校验通过也不代表目标平台的实际部署验收通过。


## 同一发行版本的多个目标

`setup/install.json` 的 schemaVersion 2 包含 `catalogs`，键为 `k3d-arm64` / `k3s-arm64` 等目标，值是仅含该目标的完整 schemaVersion 1 清单。CLI 根据实际目标先选择清单，再计算应用依赖；未提供的目标、索引不匹配或损坏清单均拒绝。schemaVersion 1 继续兼容；旧 CLI 会拒绝 schemaVersion 2，因此采用新清单必须配套发布 CLI。

多目标离线目录共享 `chentu/` 部署程序，资源分别存于 `targets/<目标>/<清单 path>`。在线缓存也按目标分目录，避免同名 Docker/OCI 归档互相覆盖。运行时 staging 仍使用清单原始 path，不改变宸途工具的路径约定。OSS 对象仍按内容摘要共享，不按目标重复上传。单目标发布器处理每个目标的独立资源目录，再由宸途 `bundle/release_catalog.py` 合并已发布清单；不能把不同目标的组件与文件直接拼成一个平面清单。

## 正式 Nexus 发布契约

新目标以 nexusFirst 能力显式启用正式分发。setup/release-manifest.json 本身必须列入安装目录的 SHA-256 清单；发布清单完整记录 release、Chentu/CLI 的确切 Git commit、工具链、仓库、profile、组件依赖及统一 artifacts。镜像分别记录来源和目标 digest，Chart 记录名称、版本、release 归属及 tgz 校验值。目标路径只使用仓库内部路径，运行时域名不进入发布版本身份。

CLI 在创建集群前验证完整发布目录、所选应用依赖、完整 Nexus bootstrap 闭包，以及实际 Chentu 源码/生成目录的逐文件 SHA-256。新版 sourceOverlay 必须为空。只接受 ONLINE hosted/proxy 仓库；proxy 必须启用 autoBlocking。联网与离线使用同一个正式 nexus_first.py 部署流程，proxy 始终 ONLINE。旧版无 nexusFirst 能力的目录继续使用原安装契约。

CLI 生成 nexus/install-selection.json，保留同一个 release 并声明所选 profile、组件闭包和 artifact ID；images/install-manifest.json 的 bootstrap 标记来自同一基础闭包。离线缺包错误逐项显示名称、版本、平台、仓库及路径。正式 Nexus 分发的管理机契约为 macOS ARM64 或 Linux ARM64 对应 `k3d-arm64 / single-k3d`、Linux AMD64 对应 `k3d-amd64 / single-k3d`，或 Ubuntu 22.04 AMD64（含 22.04.x 点版本）对应 `k3s-amd64 / multi-k3s`。CLI 使用实际管理机系统、硬件架构和运行架构核对契约；其他系统、版本、架构、安装目标或拓扑组合在资源准备前拒绝。Apple Silicon 上运行 AMD64 CLI 同样不满足 macOS ARM64 契约。

原生 Nexus-first 候选目标为 `k3s-amd64 / multi-k3s`，要求包中明确声明 Ubuntu 22.04
AMD64，节点恰为三个 server。安装目标声明描述的是部署节点；管理机还必须满足上述对应平台契约，不能由安装目标标签推断实际管理机系统。每个节点继续独立通过系统和架构检测。CLI 将同一 inventory 传入 Chentu 驱动，并只预载 bootstrap
闭包镜像；随包原生工具须含 crane。此修复仅验证管理机平台检查、正式清单/来源校验及真实资源准备路径，没有创建或修改集群。原生三节点部署、Nexus 就绪、镜像推送、Helmfile sync、OIDC、断网安装和中断恢复尚未完成本次验收；正式支持以对应发行版本的独立断网验收记录为准。

APT 与 PyPI 发布制品使用原生 hosted 坐标。CLI 分别验证 Deb 包名、版本、架构和 wheel 文件名，
只允许对应的确定性原生路径；Deb epoch 等版本字符不会放宽本地文件路径校验。两类制品必须声明 native transport，
目标架构不符、路径篡改或封装声明错误在修改集群前拒绝。

### Linux ARM64 / AMD64 hosted K3d

正式清单可声明 `k3d-arm64` 或 `k3d-amd64` / `single-k3d`。Linux 管理机的
硬件、CLI 进程与所选包必须同为 ARM64 或同为 AMD64；Mac ARM64 对应 `k3d-arm64`。
DGX Spark 的 Ubuntu 24.04 ARM64 使用 K3d ARM64 契约，不因此获得原生 K3s ARM64 支持。
Docker 的经典镜像存储和 containerd 存储可能分别返回 config 与 OCI manifest/index 摘要。
系统镜像导入后若 ID 不直接相同，CLI 只接受能从已校验归档证明对应关系的 config：
标签必须唯一，config 字节和平台必须匹配，OCI 摘要链和 config 引用必须一致，
子描述符大小逐项核对。不会因为标签存在而忽略身份不匹配。
目标、清单、资源镜像和工具箱均须匹配架构，不能通过修改目标名复用 ARM64 归档。
原生 `k3s-amd64` 仍仅接受 Ubuntu 22.04 AMD64 管理机。
K3d 验收不覆盖原生主机引导、多机存储或真实断网首装；这些结果单独记录。

### 统一三拓扑支持矩阵

正式清单中每个 profile 的 `supportedTopologies`（由 chentu 发布器写出，与安装目录 `nexusFirst.supportedTopologies` 必须一致）决定该目标可部署的拓扑；CLI 只固定“家族”：`k3d-*` 目标只能部署 `single-k3d`，`k3s-*` 目标只能部署 `single-k3s` / `multi-k3s`。拒绝文案按列表拼出，例如“此包不支持所选部署拓扑；k3s-amd64 支持：multi-k3s、single-k3s”。

| 安装目标 | 拓扑 | 管理机契约 | 节点规则（hosted-first） |
| --- | --- | --- | --- |
| `k3d-arm64` | `single-k3d` | macOS ARM64 / Linux ARM64 | `nodes: []`，`entryIp: 127.0.0.1`，runner `docker`，端口由实例注册表分配 |
| `k3d-amd64` | `single-k3d` | Linux AMD64 | 同上 |
| `k3s-amd64` | `single-k3s` | Ubuntu 22.04 AMD64（管理机即目标机） | 配置 `nodes: []`，向导探测本机后以 `localhost` 作为唯一 server，Ansible 本地连接；固定 80/443 |
| `k3s-amd64` | `multi-k3s` | Ubuntu 22.04 AMD64 | ≥3 个、奇数个 server，0 个 agent（`ha: true`）；每个节点独立通过系统/架构检测 |

`config.rs` 的通用校验保持不变（单机拓扑不接受远程节点；多机 HA 需奇数且 ≥3 个 server）；hosted-first 原生规则在资源准备后、创建集群前由 `bootstrap::hosted_nodes` 追加检查。发布器 `installer-support` 验证器与 `assemble-installer.js` 的默认配置契约同样放开三拓扑：multi-k3s 用 `192.0.2.x` 占位的 3 个 server，single-k3s 为 `nodes: []`，single-k3d 为 `nodes: []` / `127.0.0.1` / `54320`、`54321` / runner `docker`。

### 安装进程重启后继续

正式 Nexus-first 安装包在集群引导完成、首次 Helm 同步前，写入权限 `0600` 的恢复检查点。
安装尚未成功结束时，CLI 重启后显示“继续原部署”；失败或停止后也可继续已经生成检查点的安装。成功结束后检查点归档到原运行目录，不作为待恢复任务。
继续保留原运行目录、日志、集群与凭据，不按进度条跳过应用，而是重新检查 Helm 未完成操作并执行同一官方同步入口。

继续之前必须满足：CLI 二进制、配置、Chentu 源码与生成目录、所选资源、部署环境、inventory、kubeconfig 均保持检查点身份，实际集群 `kube-system` UID 一致。
检查点仅保存摘要、制品坐标和本次部署路径，不复制模型密钥或 kubeconfig 内容。
旧进程、子进程组或工具箱尚未退出、生成子进程时状态不确定、并发恢复、文件或集群变化时均拒绝继续，不自动删除 Helm pending 记录、不杀死旧进程、不接管其他集群。

检查点尚未产生的正常检查失败仍可修正后重新部署；集群引导中途硬退出、旧版本 CLI 的遗留安装、任意开发源码部署不自动恢复。
更换发行包或 CLI 属于升级，不能冒充同一次安装继续。真实业务状态和断网验收仍需独立验证，成功续跑并不代表所有业务通过。

恢复记录损坏、结构无效或文件权限不安全时，向导仍可打开并显示阻塞原因，但继续和新部署都不会覆盖检查点。
检查点 `<config>.installation-resume.json` 与恢复锁 `<config>.installation-owner` 必须是当前用户独占、无硬链接的私有普通文件。
只有人工确认旧进程组和工具箱都已停止后，才可备份移走损坏记录并明确开始新安装；向导不会自动清除或忽略旧记录。


原生随包工具通过本安装私有路径显式导出 `CHENTU_KUBECTL`、`CHENTU_HELM`。
向导的 Helm 状态/集群身份检查和模型写入使用这些绝对路径，并通过命令参数
绑定本次 kubeconfig；不会选用管理机 PATH 中遗留的 kubectl 包装器。Nexus-first
节点等待亦须由宸途将工具和 kubeconfig 显式传入 delegated localhost。恢复记录
保留原工具环境并继续核对包、文件和集群身份，不以扫描“最新 run”接管安装。

## 安装实例与共存

一个配置路径就是一个安装实例：集群工作目录、集群名（`cluster::identity`）、检查点 `<config>.installation-resume.json` 与实例锁 `<config>.installation-owner` 均由该路径派生，实例化不改变这些规则。实例 ID 是配置路径 SHA-256 的前 12 位十六进制，仅作注册表键；`installation.instanceId` 可选透传，不参与校验。

- **实例目录**：安装包 `start.sh` 使用 `~/.apeiron/installers/<candidate>/instances/<instance>/config.json`，`APEIRON_INSTANCE`（默认 `default`，小写 DNS 标签）选择实例，`APEIRON_WIZARD_PORT`（默认 3210）选择向导端口；随包 Python 运行时共享于 `installers/<candidate>/bootstrap`，缓存、配置与数据按实例隔离。`apeiron init --instance <name>` 使用 `~/.apeiron/instances/<name>/config.json`，与 `--config` 互斥。向导内“创建新安装实例”在当前配置所在的 `instances/` 根（或 `~/.apeiron/instances/`）下建目录。
- **向导首屏**：两个入口“创建新安装实例”（名称 + 拓扑，服务端分配端口后进入配置流程）与“管理已有安装实例”（列表：ID、名称、拓扑、域名、端口、状态 `phase`/`k3d`；操作：选择/继续、诊断采集（下载 `api/log/download`）、卸载（仅 K3d 且集群未运行、非当前实例））。页眉实例标签与第 3 步始终显示当前正在操作的实例与配置路径。创建流程顺序：拓扑 → 应用与场景组合 → 共存配置与资源预算（显示 `api/preflight` 结果）→ 域名 / 访问方式 / 既有模型服务 → 预检与部署摘要 → 开始安装；入口端口只读展示。
- **注册表**：`~/.apeiron/instances.json`（`schemaVersion: 1`，0600，临时文件 + rename 原子写）记录 `id`、`name`、`topology`、`config`、`httpPort`、`httpsPort`、`createdAt`。格式损坏、ID 与路径不符、名称或端口重复时拒绝读写，不自动修复。向导打开时若当前配置位于 `instances/<name>/` 布局下而未登记，会按配置中的端口自动登记一次。
- **端口分配**：新建 K3d 实例时从 54320 起、步长 2 选择 HTTP/HTTPS 一对端口：排除注册表已保留端口，对两个端口各做一次 `127.0.0.1` 的 `bind` 探测，并解析 `docker ps -a --format '{{.Ports}}'` 排除已停止的 k3d 负载均衡容器仍持有的绑定。Docker 不可用时不创建 K3d 实例。K3s 实例固定 80/443。
- **调度锁**：`~/.apeiron/scheduler.lock`（flock，当前用户私有普通文件）覆盖“分配端口 → 写入配置 → 登记”和部署的 `cluster` 阶段（端口复查与 `k3d` 创建）。部署等待锁时写入事件并最多等待 30 分钟；实例创建与删除最多等待 30 秒后报错。每实例锁继续沿用 `installation-owner`，检查点路径不变。
- **SSH 转发**：向导仍只监听 `127.0.0.1`。Host 白名单为 `127.0.0.1:<任意端口>`、`localhost:<任意端口>` 及显式 `--advertise-host <host[:port]>`；POST 的 Origin 必须等于 `http://<请求 Host>`，跨源与 `Sec-Fetch-Site: cross-site` 仍拒绝。远程使用 `ssh -L 3210:127.0.0.1:3210 <user>@<host>` 后通过 `http://127.0.0.1:3210/<打印的路径>` 访问；启动输出的 `forward` 行给出该命令。
- **实例接口**（同源 JSON，与其他 API 相同的 Host/Origin 规则；活动部署、访问配置或测试进行中时拒绝变更）：
  - `GET api/instances` → `{registry, path, current, instances:[{id,name,topology,config,httpPort,httpsPort,createdAt,exists,phase,locked,cluster,k3d}]}`；`phase` 取自恢复记录（`new`/`idle`/`interrupted`/`succeeded`/`blocked`），`k3d` 由 `docker inspect k3d-<cluster>-serverlb` 得出（`running`/`stopped`/`absent`/`unknown`，K3s 为 `null`）。
  - `GET api/instances/<id>/status` → 单个实例的同一结构。
  - `POST api/instances/create {name, topology[, select]}` → `{instance, existing, selected}`：分配端口、从当前配置复制应用选择与离线包路径生成 `<name>.apeironlab.internal` 的模板配置（多机 K3s 使用文档占位节点），写入并登记。`select: true` 时同时切换并附带 `select` 的返回字段。若 `name` 就是向导当前所在、已登记的实例（`start.sh` 用 `APEIRON_INSTANCE` 打开时的情形）且拓扑一致，则幂等返回 `existing: true`；拓扑不一致 409；其他重名 409。每个实例状态额外含 `domain`（取自其配置，仅展示）。
  - `POST api/instances/select {id}` 或 `{name}` → `{instance, path, config, revision, deployment}`：无活动部署时切换向导当前配置路径并重新读取该实例的恢复状态；网关向导不支持。
  - `POST api/instances/delete {id}` 或 `{name}` → `{deleted, clusterDeleted, removedDirectory}`：仅 K3d、非当前实例、集群未运行；集群已停止时执行 `k3d cluster delete <cluster>` 与 `k3d registry delete k3d-<cluster>-reg`，再删除实例目录并注销。运行中、状态未知、被其他进程持有或恢复记录损坏时拒绝；名称不存在 404；从不执行 `docker prune` 或泛化删除。
  - `GET api/config` 额外返回 `instance`（当前配置对应的注册表记录状态，未登记为 `null`）与 `path`。
  - `POST api/preflight`（请求体同 `api/deploy`）→ `{preflight, blocked, installation}`：按 `api/deploy` 规则校验但不保存，执行同一共存/端口预检；可修正的拒绝原因（4xx）以 `blocked` 字符串返回而非错误状态。
- **共存预检矩阵**（本机目标在点击部署、重试和创建集群前执行；多机节点由节点探测负责）：

| 宿主状态 | 单机 K3d | 单机 K3s | 多机 K3s 节点 |
| --- | --- | --- | --- |
| 已有 `/var/lib/rancher/k3s` 或 `/etc/rancher/k3s` | 放行并警告，入口使用分配的高位端口 | 拒绝，不接管或覆盖 | 拒绝（节点探测 `existingCluster`） |
| 已有 `k3d-*` 容器 | 按归属记录复用或新建，不接管无归属集群 | 警告（节点探测 `existingK3d`） | 警告 |
| `ip route` 的 10.42/10.43 与 Docker 网络子网重叠 | 警告 `routeOverlap: overlap` | — | — |
| `iptables -S DOCKER-USER` / `FORWARD` | 最佳努力读取；失败记 `unknown`，`FORWARD` 默认 DROP 时警告 | — | — |

预检只读取状态，不修改路由、防火墙或 Docker 资源；警告随部署事件写入安装日志并在 `api/deploy` 响应的 `preflight` 字段返回。多实例、共存与 SSH 转发尚未在真实多实例宿主上完成验收。

快速切换共存与摘要步骤时，向导等待正在执行的预检结束，再检查最新配置；过时结果不更新页面。
浏览器取消请求不会取消服务端 Docker 检查，因此不以中断 fetch 代替完成等待。
开始安装和返回实例首页也等待该检查结束，避免与服务端的保存/配置互斥锁竞争。

### 验收 harness 契约（WP5 对齐）

- `start.sh`：读取 `APEIRON_WIZARD_PORT`（默认 3210）与 `APEIRON_INSTANCE`（默认 `default`），以 `--config "$STATE/config.json" --port "$WIZARD_PORT"` 启动，并透传其余参数（如 `--no-open`）；CLI 标准输出保持 `schema=apeiron.init.v1` 表格：`url\thttp://127.0.0.1:<port>/setup/<hex>/`、`config\t<path>`、`forward\tssh -L <port>:127.0.0.1:<port> <user>@<this-host>`。
- 远程使用：工作站执行 `ssh -L 3210:127.0.0.1:3210 <user>@<host>`，然后打开 `http://127.0.0.1:3210/<url 行中的路径>`；POST 的 `Host` 与 `Origin` 均为 `http://127.0.0.1:3210`。
- 部署请求体 = `GET api/config` 的 `config` 叠加 `deployment.installation{topology,domain,entryIp,httpPort,httpsPort,ha,nodes,sshUser,sshKey,sshPort}`、`apps` 与 `revision`；K3d 的端口须与注册表登记一致（创建实例时由服务端分配）。
- `GET api/deployment.stages[]` 元素为 `{id, state, detail}`，`id ∈ check/cluster/nexus/import/applications/access`，`state ∈ pending/active/done/skipped/failed/cancelled`；中断用 `POST api/deployment/stop {}`，续跑 `POST api/deployment/resume {revision}`，重跑 `POST api/deployment/retry {revision}`。
- 应用目录随安装包发布（`install.json` 的 `applications`，来自 Chentu `deploy/helmfile/applications.yaml`）；向导在第一步选定安装包后用 `POST api/applications {deployment}` 读取。不在目录中的 ID（如 `keycloak`）被 `安装包的应用目录不包含：<id>。` 明确拒绝；资源目录缺少对应组件时在 `check` 阶段以“安装包缺少组件：<id>”拒绝。

访问页读取 `keycloak/keycloak-operator` 的持久工作空间账号（默认 `platform-owner`），
不把初次 IAM 管理引导账号作为应用登录账号。已修改 IAM 密码时使用新密码；
应用角色与成员权限继续生效。首次安装明确选择 Files 时，向导默认声明
`releases.files.values.operatorBucket: files-owner`，由宸途绑定到实际持久操作员，提供私人桶与
已有共享桶 `files-team`；保留已有 `users` 和显式桶配置，不自动开通其他 IAM 用户。

K3d 的 kubeconfig 由 CLI 预先创建为当前用户拥有的 `0600` 普通文件，工具箱原位写入；
这样 Linux Docker 不会留下 CLI 无权读取的 root 专有文件。重试保留已有文件与凭据，
拒绝符号链接、特殊文件和不可读文件，不扩大读取权限。

### 统一 IAM 制品来源

统一 IAM 产品的发布 catalog 使用 `preparationSource` 约束镜像准备路径：
Keycloak 是 `public-required`，来源由 chentu 的已提交契约锁定到官方 Quay 仓库；
IAM UI 是 `oss-only`，沿用匹配发行包的 OSS 文件下载。公网镜像准备失败不回退到 OSS。
CLI 调用已校验 chentu 源码包中的 OCI 验证器，同时校验回执来源、digest、目标架构、
归档 SHA-256/size 和实际文件字节，成功才原子写入缓存。已经验证的缓存可复用；
离线模式只接受完整缓存，不联网准备缺失镜像。

此功能需要同时发布匹配的 chentu、IAM Chart 与 CLI。旧发行版不会因本地修改自动
切换来源。Chart 与 UI 制品在 OSS 发布，部署时镜像仍供给到站点 Nexus；来源选择
发生在管理机准备阶段。
