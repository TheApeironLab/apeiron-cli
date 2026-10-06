# Apeiron CLI

统一入口：`apeiron <模块> <命令>`。`apeiron init` 提供内置本地部署向导，`apeiron onto` 接入现有 Ontology CLI。TypeScript strict + Bun 1.3.14+；onto 模块需要已安装依赖的 ontology 仓库。

## 本地安装

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

命令监听 `127.0.0.1` 的空闲端口，自动打开网页。setup 分为两步：

1. **组织与部署环境**：Slug、宸途仓库路径、环境 values 文件，以及部署方式对应的目标信息。
2. **应用选择**：确认应用后点击“开始部署”，生成环境副本并立即运行宸途的 `Helmfile sync`。

模型连接页面已移除。既有环境 YAML 内的模型声明继续由宸途读取、校验；向导不新增或修改模型凭据。

| 选择规则 | 应用 |
| --- | --- |
| 必选，不可取消 | Vasi、Apeiron（含 Ops）、Limani、Task、Corpus、Chat（团队聊天）、Nexus、邮件 |
| 默认选中，可取消 | Files、Gateway |
| 默认不选 | Filer、代码仓库、GPUStack、Langfuse、Grafana |

默认选中 10 项，其中 8 项必选。展示顺序仍为 Vasi、Apeiron、Limani、Task、Corpus、Chat、Files、Gateway、Nexus、Filer、邮件、代码仓库、GPUStack、Langfuse、Grafana。
Nexus 与邮件分别满足平台应用和 Task 的现有依赖。重新打开保留可选应用选择；旧配置中缺少的必选应用会自动勾选，提交时才写入。

### 部署目标

页面默认面向 K3s 部署，使用 ubuntu profile。第一步底部的“开发测试”默认折叠；展开后可勾选“本地测试（k3d）”，显示工作目录和工具箱镜像，并在页头持续标识测试模式。不需要 CLI flag。

所有路径均指运行 CLI 的机器；环境文件必须是仓库外的普通 YAML。setup 收集以下字段，不要求预先设置环境变量：

| 方式 | 字段 | 实际执行入口 |
| --- | --- | --- |
| K3s 部署（默认） | 宸途仓库、环境文件、kubeconfig | `bash deploy/helmfile/run.sh sync` |
| 本地测试（k3d） | 宸途仓库、环境文件、外部工作目录、工具箱镜像 | `bash tests/lab/helmfile.sh sync` |

K3s 部署使用指定 kubeconfig 的 current-context，需要 PATH 内有 Helmfile、Helm 及宸途所需工具。
本地测试使用 Docker 工具箱与已准备的 k3d 集群和镜像，工作目录内必须有 `state/kubeconfig`；环境中的 `/work` 路径、集群名和网络等沿用宸途 lab 约定。
本向导接入应用同步；主机、K3s/k3d 集群、私有镜像、离线制品、存储与域名需要事先准备。

`APEIRON_CHENTU_ROOT`（或 `CHENTU_ROOT`）、`CHENTU_ENV`、`KUBECONFIG`、`CHENTU_PROFILE`，以及 `LAB_ENV`、`LAB_WORK_DIR`、`LAB_IMAGE` 可预填配置；已有保存配置优先。环境变量不会自动开启本地测试。已保存的本地测试配置会恢复勾选、展开测试设置并显示标识；已有 native/local 配置保留原 profile，避免自动改变存储设置。
其他部署环境变量沿用启动 CLI 的终端，例如 `CHENTU_PYTHON`、`LAB_CLUSTER`、`LAB_NETWORK`、`LAB_HELMFILE_BIN`。

### 配置与进度

每次部署在配置目录的 `deployments/run-*/` 生成 `environment.yaml` 与 `helmfile.log`。
生成器只改一个外部环境文件副本中的 `tenantSlug` 和应用 `releases.<id>.enabled`，保留已有 values、镜像、存储和模型设置；默认值与 profile 的合并、依赖校验、安装顺序和 hooks 全由宸途与 Helmfile 处理。

Limani 对应 `ontology`，Chat 对应 `matrix`，邮件对应 `stalwart`，代码仓库对应 `git`。Grafana 选项控制宸途 observability 组件的 `kps`、`loki`、`promtail` 三个 release；其余 ID 为小写应用名。Apeiron 包含 Ops，无需单独选择。
取消选择只将相应 release 排除于本次同步，不卸载已有应用；平台访问权限仍由平台管理。

网页展示部署状态、耗时、退出码、已识别的同步进度与日志路径。完整输出仅写到本机日志，可能含部署诊断与敏感配置，使用 `0600` 权限，不发送到网页或终端。
重复启动会被阻止，刷新页面会恢复当前进度。同一源环境文件的多个向导共享部署锁。
失败后可返回修改并重试。重试重新运行 sync；已完成的部署、数据库与外部系统变更不会自动回滚。

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

页面、脚本、样式与官方 Apeiron logo 直接包含在 CLI，无 CDN、独立前端服务或 LLM 依赖。
会话使用随机访问路径，写入接口校验同源请求。部署结束后点击“完成并关闭向导”，或在终端按 Ctrl+C 停止进程与本地服务。
仅关闭网页不会停止部署。远程使用可通过 SSH 转发端口。

## 独立可执行文件

```sh
bun run build
./dist/apeiron init
```

构建生成当前系统架构的可执行文件，包含本地配置页面和 Bun 运行时，使用向导无需在目标机器安装 Bun。
`onto` 转发仍需要其源码/依赖或配置好的外部 CLI。构建产物不提交到 Git；暂未发布跨平台下载包。

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
用隔离的部署替身验证两步配置、必选项、失败/重试、刷新恢复、完成退出、桌面/手机布局以及没有外部网络请求。它不对真实集群执行 sync。
浏览器测试只用虚构配置，完成后删除配置和替身日志；截图保存在打印出的系统临时目录。

真实 Helmfile 配置读取与编排检查（需要已有宸途工具箱镜像，不挂载 Docker socket 或 kubeconfig，不连接集群）：

```sh
bun scripts/test-helmfile.ts /absolute/path/to/chentu chentu-lab
```

该检查运行原生 `print-env` / `build`，校验启用项、values 保留与依赖完整性，不执行真实集群部署。

当前尚未发布到 npm registry。顶层 `status` / `verify` 仍沿用早期 Ontology 入口检查含义；
`platform init` 已由顶层 `init` 的网页流程替代；部署由顶层 init 调用宸途原生入口，未另设 `platform install/deploy` 命令。
