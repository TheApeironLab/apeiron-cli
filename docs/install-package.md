# 宸途全新安装资源契约

CLI 当前仍固定下载 OSS `0.1.0-rc.4`，但该包不符合新版 `deploymentTopology` / `clusterOidc` 契约，会在创建集群前被拒绝。发布兼容包并更新 CLI 固定版本之前，开发联调需显式提供兼容的本地包。旧包包含 `k3d-arm64` 目标、部分应用资源、工具箱及系统镜像归档；基于 PR #87 提交，并以 `chentu-package.json.sourceOverlay` 逐文件记录修复摘要。其他目标和未提供资源的可选应用在变更集群前拒绝。

发行包内 `setup/install.json` 使用 `schemaVersion: 1`，包含：

- `targets`：`k3s-amd64`、`k3d-amd64`、`k3d-arm64` 等已验证目标；每个目标指定资源组件 `base`、可部署的 `environment` 和 K3d 所需的 `toolboxImage`（固定 SHA-256 digest），或 `toolboxArchive` + `toolboxImageId`（已校验归档及实际 Docker 镜像 ID）；`dockerArchives` 记录 K3d 系统归档和各镜像名称／ID。未提供的目标不能安装。
- `components`：按应用 ID 对应资源组件，字段 `requires` 是资源依赖，`files` 是文件清单中的路径，`values` 是合入发行包基础环境的固定配置。每个用户选择的应用必须有记录。`images` 将归档映射到内部 registry 引用及 manifest digest；同一引用不允许冲突版本。此接口不排序 Helm releases、不安装 Charts，也不代替 Helmfile 的依赖真相源；发行端应从解析后的 Helmfile 与 artifacts 生成并验证它。
- `files`：每项具有安全相对 `path`、正整数 `size`、64 位十六进制 `sha256` 和可选 HTTPS `url`。不接受目录穿越、符号链接或重复路径。下载 URL 是实际制品地址／代理地址；不会检查注册表根路径后就宣称制品存在。预期的字节来自固定发布版本，不能运行时信任一个刚下载文件的自身散列。

每个 target 必须声明 `deploymentTopology: true`，其环境与部署入口使用 `topology`，不接受旧 `profile`。Vasi 必选，因此目标还必须声明 `clusterOidc: true`，资源依赖包含 `cluster-access`，部署程序包含 `cli/src/chentu/cluster_oidc.py` 和 `bootstrap/oidc.yaml`。这些标记代表发行包实际实现的能力，不能仅修改清单跳过检查。

环境字符串可使用 `__DOMAIN__`、`__REGISTRY__`、`__NODE__`、`__WORK__`、`__BUNDLE__`、`__ARCH__`。这些值在 CLI 生成外部 YAML 时替换。具体应用镜像、模型默认声明、skills bundle 等必须由发行方提供可部署值；占位版本或 `unset` 应被宸途检查器拒绝。模型配置可以通过发行包显式声明延后，这不代表模型对话已经可用。

K3s 资源需包含兼容的系统包、K3s 二进制／安装脚本／airgap 镜像／审计策略、CLI wheels、工具、Charts、应用镜像 tar 与 `images/manifest.txt`、`images/nexus-names.txt`。镜像 manifest／架构／digest 的一致性应在发布时完成验证，不能只把任意文件列入清单。必要的上游镜像包括 docker.io、ghcr.io、quay.io，以及启用相应组件时的 codeberg.org、registry.k8s.io、nvcr.io；自研应用的私有镜像同样必须显式纳入。安装时资源必须全部在本地、匹配大小和 SHA-256 后才能继续。

在线模式接受 HTTPS 重定向（最多 5 次、不携带凭据），完整下载到临时文件，校验后原子替换缓存。若下载需要身份，发行方需提供可直接访问的制品地址；当前该资源接口不会自动继承 GitHub、Docker 或任意域名的凭据。宸途部署程序使用独立的公开 OSS 下载器，仅允许固定发布目录，并由 CLI 内置 SHA-256 与源码提交校验。

离线目录包含 `chentu/` 部署程序及其 `setup/install.json`，资源位于目录下的清单相对路径。目录选择授权使用其中的部署程序，应来自受信任的发布方。离线接口不调用网络下载；不支持当前需要外网准备的 K3d lab 流程。选择的文件被复制到独立工作目录，再次校验并生成本次专用 `SHA256SUMS`，因此清单不得自行列入 `SHA256SUMS`。

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
