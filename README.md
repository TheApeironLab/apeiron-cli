# Apeiron CLI

部署核心与安装网页由 `third_party/chentu/rust` 对应的 Chentu 源码持有；本仓库只嵌入该 crate 并适配命令。旧 `apeiron init/app/platform` 暂保留兼容。维护边界与验证见 [Chentu 宿主集成](docs/chentu-host.md)。

统一入口：`apeiron <模块> <命令>`。`apeiron chentu init` 提供内置本地部署向导，`apeiron onto` 接入现有 Ontology CLI。Rust 单文件程序，内嵌浏览器向导；运行不需要 Bun/Node。onto 模块需要外部 CLI 或已安装依赖的 ontology 仓库。

`apeiron sudo` 启动随发行版交付的 Sudocode 本地 AI agent，参数直接传给 `scode`。
配置模型 base URL、API key 和模型即可使用，无需 Gateway 或 Apeiron Backend；agent 也能通过
原生 CLI 工具目录发现并调用 `apeiron`。交互、脚本模式、配置和离线交付见 [Sudocode 入口](docs/sudo.md)。
源码通过 `third_party/sudocode` 子模块固定；发行时打包对应的预编译 `scode`，更新子模块后用
`scripts/pin-sudocode.js` 生成制品清单，下载源可使用上游或 OSS，详见上述更新说明。

## 安装发行版

macOS / Linux，ARM64 或 x64：

```sh
curl -fsSL https://downloads.apeironlab.cn/apeiron-cli/install.sh | sh
apeiron chentu init
```

安装器自动选择平台并校验 SHA-256，优先原位更新已能找到的 `apeiron`，否则选择当前 PATH 中的常用 bin 目录。用户目录可直接写入；系统目录不可写时会请求 sudo 授权。安装后当前终端即可直接运行 `apeiron`，无需手动修改 PATH。`apeiron chentu init` 自动打开默认浏览器。重新运行安装命令可升级 CLI，不会重新部署平台或修改配置。独立文件也可从 GitHub Releases（需要仓库权限）或公开 OSS 下载；离线安装和固定版本见 [发布与安装说明](docs/releases.md)。

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
git clone --recurse-submodules https://github.com/TheApeironLab/apeiron-cli.git
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

## 基础站点初始化

```sh
apeiron chentu init
```

安装向导及部署规则由固定的 [Chentu 源码](third_party/chentu/rust/README.md) 提供。
新流程为 **机器 → 站点 → 检查 → 安装 → 进入 Vasi**：先创建 K3s/K3d、引导 Nexus、
供给固定材料，再启动 IAM 和 Vasi，完成站点交接。首装不选择业务应用或模型。
后续应用管理在 Vasi 完成；IAM 管理成员和授权。

新向导必须使用包含原生 release catalog、Chentu 管理执行器与 Vasi 的完整 v2 基础站点材料。
仓库当前固定的旧发行包尚不具备此能力；更新 CLI 不会把旧包变成新包，也不会回退到 Minimal。
开发验收在向导中明确选择已准备的配套材料；正式默认包待发布和目标验收后再更新。
材料支持的平台、在线准备与断网验收范围均以该包声明为准。

`.apeiron` 配置、实例索引及缓存继续兼容；不迁移或重写已有凭据。旧 `apeiron init/app/platform`
只是同一 Chentu 模块的兼容入口。安装参数、恢复与日志见
[Chentu CLI](third_party/chentu/rust/README.md) 和
[站点生命周期](third_party/chentu/docs/stack/site-lifecycle.md)。

## 独立可执行文件

```sh
cargo build --release --locked
./target/release/apeiron chentu init
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

构建输出和测试依赖放在仓库外。先初始化固定子模块，再按 Chentu 文档准备其外部 UI 依赖：

```sh
export CARGO_TARGET_DIR=/external/apeiron-target
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -p apeiron -p chentu
cargo build --locked --bin apeiron --example build-support
bun scripts/embed-assets.js --check
bun run check
CHENTU_EMBEDDED=1 APEIRON_TEST_BIN="$CARGO_TARGET_DIR/debug/apeiron" bun test
APEIRON_TEST_BIN="$CARGO_TARGET_DIR/debug/apeiron" bun run test:ui
python3 -m unittest discover -s test -p 'test_*.py'
```

`test/chentu-contracts.test.js` 自动加载固定 Chentu 的全部 HTTP 契约，用内嵌入口执行原断言。
浏览器套件清单也由 Chentu 维护，本仓库只设置宿主模式；不复制安装页面或另列旧 Minimal/模型测试。
命令路由、旧别名、选定材料和 build-info 的集成断言保留在 `test/chentu.test.js`。
浏览器测试为隔离替身，不代表真实部署；真实部署仍需比较凭据、PVC 和数据。

材料组装、内容地址校验和冻结实现位于 Chentu `bundle/release/`；`scripts/` 同名 Python
入口仅调用固定子模块。apeiron-cli 保留跨产品发行协调、签名、上传和通道验收，相关集成测试
仍在本仓库。先前 Minimal 工具仅用于旧材料兼容，不能作为新基础站点的发布入口。

## 公网入口配对（新发行包）

已有 Ubuntu ECS 安装 Caddy 2.10+、OpenSSH、sudo 和 Python 3 后，在 ECS 生成配对码：

```sh
sudo apeiron platform entry pair --domain team.example.com --public-ip <公网IPv4>
```

在 Spark 等部署主机运行 `apeiron chentu init`，选择 ECS 转发，粘贴完整配对码。配对码 10 分钟有效，只绑定一个设备；同一设备可在有效期内重试。配对后不需要提供 ECS 管理员 SSH 私钥，域名与入口地址由配对记录限定。配对码含临时认证材料，不要分享或写入日志。

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

Zygos 在基础站点就绪后通过 Vasi 安装，需要配套应用材料。GPU 节点与模型权重单独配置。

## 站点交接后的应用管理

已交接站点由 Vasi 调用 Chentu API 安装、更新和卸载应用；操作与记录保存在站点管理节点，
关闭页面不停止执行。更新使用现场固定材料，不代表自动发现新版本。

`apeiron chentu app` 保留旧的未交接实例兼容；已交接实例会拒绝使用安装机上的过期配置修改站点。
不要删除交接标志来绕过保护。故障恢复使用管理节点上的 Chentu CLI 和原生工具，详见
[Vasi RUNBOOK](third_party/chentu/stack/vasi/RUNBOOK.md#站点管理模式开发预览)。
平台整体升级、备份恢复及灾难恢复不因页面提供应用操作而自动具备。

## Zygos 内置子命令

`apeiron zygos` 静态链接 Zygos Rust CLI，单个 apeiron 文件即可使用；
不需要外部 zygos 可执行文件，也不读取 `APEIRON_ZYGOS_BIN`。

```sh
apeiron zygos --help
export ZYGOS_API_BASE_URL=http://127.0.0.1:8000
apeiron zygos model list
```

连接、认证、JSON 输出、字段选择和退出码沿用 Zygos CLI。API 服务仍需单独部署。
CI 使用的 `ZYGOS_READ_TOKEN` 需要同时能读取本仓库、Zygos、Apeiron、Apeiron IAM 与 Chentu；名称沿用现有配置。

源码通过 `third_party/zygos` Git submodule 锁定确切提交，Cargo 直接引用其中的
`apps/cli`，不复制业务 CLI 源码。开发与更新方式见 [Zygos 子模块](docs/zygos-submodule.md)。
克隆开发仓库后先运行 `git submodule update --init --recursive`，最终用户不需要此步骤。

## 本机连接运行时

`connect` / `status` / `disconnect` / `serve` 和 `cloud-receiver` 由本仓库 `runtime/` 维护，
接收端与 `apeiron --version` 使用同一个 CLI 发行版本。`third_party/apeiron` 仅记录部署发行的
服务端源码版本，不再提供 CLI 的 Cargo 依赖。协议变更需配套验证
[Apeiron 服务端契约](https://github.com/TheApeironLab/apeiron/blob/main/backend/AGENTS.md)。

在 Apeiron 的“设置 → 本地电脑”获取连接命令，或使用该平台签发的个人访问令牌：

```sh
apeiron connect --server https://apeiron.example.internal --token-file /absolute/path/to/token
apeiron status
apeiron disconnect
# 在当前终端运行，不安装后台服务
apeiron serve --server https://apeiron.example.internal --token-file /absolute/path/to/token
```

`connect` 在 macOS 使用 launchd、在 Linux 使用 systemd 用户服务；`serve` 前台运行。
服务凭据保存在 `~/.config/apeiron/token`，权限为 0600，日志位于 `~/.local/state/apeiron/apeiron.log`。
Linux 注销后持续运行需要为当前用户启用 linger。服务端吊销连接令牌后，接收端退出并卸载对应后台服务。

工作区用 `--root` 指定，默认 `~/.apeiron/workspaces`；`--config-home` 默认为 `~/.apeiron/sudocode`。
接收端优先启动托管安装器放在 `~/.apeiron/bin` 的 scode，否则使用 PATH 中的 `scode acp`；可用
`--agent` 指定命令。agent 使用当前用户的权限和模型配置，配置与工作区应由用户明确选择。
本仓库保留真实可执行文件的生命周期、TLS、Task 和云端容器契约测试；服务端另行验证连接与执行协议。

## 安装材料与发行边界

新基础站点要求和验收范围以 Chentu 文档及安装清单为准。旧 Minimal 配置、源码快照和
已发布材料仍保留原身份；本次命令适配不更新 `release/distribution-input.json` 或发行通道。
先发布并验收配套基础站点材料，再更新默认包，不能仅修改清单中的发行版名称。

平台包的 release、依赖、版本和预算来自原生 Helmfile 及其渲染结果。apeiron-cli 不维护
第二份应用列表、排序或安装逻辑。统一发行协调的输入与输出见 [发布说明](docs/releases.md)。
