# 发布与安装

> 统一发布编排的当前改动见 [统一发布编排](unified-release.md)。该入口接管标签构建后的安装验收及发布；
> 上游镜像/部署包自动构建和全部专用验收环境尚未接通，因此当前不能按下文旧流程单独发布 CLI。
> 本文其余手工发行步骤保留作原流程参考，迁移完成前不要据此创建 `0.2.1` 正式标签。

主分发渠道为公开 OSS，统一下载入口为 `https://downloads.apeironlab.cn`；GitHub Releases 保存相同文件和发行记录。GitHub 仓库仍为私有，用户从 OSS 安装无需 GitHub 账号。

## 下载域名配置

`downloads.apeironlab.cn` 绑定到北京 OSS bucket `apeiron-bj-cli-downloads`，所有对象路径保持不变：

```text
类型：CNAME
主机记录：downloads
记录值：apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com
```

DNS 记录值不包含 `https://` 或路径。还需在 OSS 的自定义域名设置中绑定 `downloads.apeironlab.cn`，并配置覆盖该域名的有效 HTTPS 证书。仅添加 DNS 记录不足以完成 HTTPS 访问；证书续期后也需更新 OSS 绑定。

HTTPS 使用 Let’s Encrypt。`Downloads HTTPS certificate` 工作流每天北京时间 11:17 检查 OSS 中的证书，剩余不足 30 天时通过 HTTP-01 重新签发并更新绑定。验证文件只写入 `.well-known/acme-challenge/`，验证后删除。工作流单独运行，不依赖 CLI 构建或私有子模块。

续期使用专用 RAM 角色 `apeiron-downloads-tls`，通过 GitHub OIDC 获取临时凭据。仓库变量 `DOWNLOADS_TLS_ROLE_ARN` 指向该角色，并复用 `OSS_OIDC_PROVIDER_ARN`。`DOWNLOADS_ACME_ACCOUNT` 是 GitHub Actions 加密 secret，仅保存复用签发账户所需的 ACME 账户资料；证书私钥只保留在权限受限的临时目录并通过 OSS API 上传，不写入 Git、公开对象或 Actions artifacts。工作流结束时删除临时目录。

可在 Actions 手动运行工作流检查续期；证书尚有 30 天以上有效期时不会重复签发。`force_renewal` 仅用于故障恢复或完整验证，避免反复使用触发 CA 签发限额。失败通过 GitHub Actions 通知维护者。更换仓库、分支或 OIDC subject 时，需要同步更新角色信任策略。证书客户端依赖固定在 `scripts/downloads-tls-requirements.txt`。

首次使用新域名发布前，确认 `/apeiron-cli/install.sh`、`/apeiron-cli/latest.txt` 及其指向版本的 `SHA256SUMS`、归档均可通过新域名下载。CLI 发布脚本和资源发布脚本使用新域名回读校验，上传目标仍为原 OSS bucket。域名未就绪时不能发布。

保留原 OSS 域名的访问能力，兼容已安装的 CLI 和已发布清单中的旧地址；不要改写已有发行版的清单或摘要。新 CLI 的部署程序下载地址和新生成的资源清单使用统一域名。

## 用户安装与更新

```sh
curl -fsSL https://downloads.apeironlab.cn/apeiron-cli/install.sh | sh
apeiron init
```

安装器使用当前终端已经包含在 PATH 中的目录，因此安装结束后直接运行 `apeiron init` 即可，向导自动打开默认浏览器。

已存在 `apeiron` 时，原位更新常用 bin 目录中的命令，保留父 shell 已缓存的命令路径；符号链接会被替换，链接原本指向的文件不变。首次安装选择 PATH 中可写的 `~/.local/bin`、`~/bin`、`/opt/homebrew/bin` 或 `/usr/local/bin`，缺失的用户 bin 目录会自动创建。不会把程序放入临时 PATH 目录、项目目录或版本管理器的 shims。

没有可写目录时，可使用 PATH 中的 `/usr/local/bin` 或 `/opt/homebrew/bin`，下载、校验并试运行通过后才请求 sudo，仅授权最后的原子安装。缺少 sudo、授权失败或没有合适的 PATH 目录时返回失败，保留旧命令。默认安装不修改 shell 配置文件。

再次运行安装命令更新 CLI；不会修改安装配置、创建集群或升级已经部署的应用。指定版本：

```sh
curl -fsSL https://downloads.apeironlab.cn/apeiron-cli/install.sh -o /tmp/apeiron-install.sh
sh /tmp/apeiron-install.sh --version 0.2.0
```

安装器自动识别 macOS/Linux、ARM64/x64，下载对应 tar.gz，校验 SHA-256，再验证新二进制能够运行且版本一致，最后替换旧文件。下载或校验失败保留旧安装。显式 `--install-dir /absolute/path` 或 `APEIRON_INSTALL_DIR` 可选择其他目录；自定义目录若不在当前 PATH 中，需自行配置访问方式。`--no-modify-path` 禁止自定义安装补写 shell 配置，`APEIRON_DOWNLOAD_BASE` 可指定具有同样目录结构的 HTTPS 内网镜像。

新版安装器默认读取 `channels/stable.txt`；`--channel nightly` 或 `APEIRON_CHANNEL=nightly` 主动选择开发版。`--version` / `APEIRON_VERSION` 跳过通道发现；与显式通道参数（包括环境变量）同时使用时报错。未知通道、缺少通道或下载失败都会停止，不会回退到另一个通道。

`latest.txt` 是 stable 的兼容入口，不是“最近一次构建”。迁移前线上旧安装器和 `latest.txt` 可能仍指向 `0.1.0-rc.4`；仅合并本次代码不会更新它们。首次 `0.2.0` 正式发布完成后才切换公共安装器与默认入口。首次 stable 前，nightly 不覆盖旧公共安装器，可从不可变版本目录安装：

```sh
# 将版本替换为已成功发布的 nightly 版本。
version=0.2.0-nightly.20261022.123
curl -fsSL "https://downloads.apeironlab.cn/apeiron-cli/releases/$version/install.sh" -o /tmp/apeiron-nightly-install.sh
sh /tmp/apeiron-nightly-install.sh --version "$version"
```

macOS 文件尚未做 Developer ID 签名和公证；浏览器下载后如被 Gatekeeper 拦截，应在系统设置中核实来源后允许运行，不要求全局关闭 Gatekeeper。

## 远程或离线

服务器无桌面时，在服务器运行：

```sh
apeiron init --no-open --port 3210
```

本机建立 SSH 转发 `ssh -L 3210:127.0.0.1:3210 user@server`，再用本机浏览器打开服务器打印的完整 setup URL。页面操作的文件、Docker 和系统配置属于运行 CLI 的服务器。

离线安装：从联网机器下载对应的 `apeiron-<version>-<os>-<arch>.tar.gz` 与同目录的 `SHA256SUMS`，拷贝到内网。用 `sha256sum`（Linux）或 `shasum -a 256`（macOS）核对该归档，再解压，将 `apeiron` 放入 PATH。运行 CLI 不需要 Bun/Node.js。

CLI 二进制不包含平台镜像。离线部署还需要对应目标的完整 Chentu 包：`chentu/` 部署程序、所选应用及依赖的镜像、Chart 和工具。向导选择“离线部署”并填写 CLI 主机上的包目录。K3d 离线部署需包含匹配版本的 K3s 基础镜像包；完整应用的断网部署尚需验收。原生 K3s 同样需对应目标的完整离线包。

当前 Chentu rc.5 提供 K3d ARM64 安装目标；原生 Ubuntu、多节点和 AMD64 需要另外发布经过验收的部署包。K3d 需要 Docker，其他应用转发命令需要单独配置相应 CLI。

## 维护者发布

### 版本与通道

根 `Cargo.toml` 保存目标基础版本，当前为 `0.2.1`，同步维护 `Cargo.lock` 中宿主 crate 的版本。补丁版本（如 `0.2.1`）保持兼容；`0.x` 期间不兼容变更放到次版本并说明迁移方法。apeiron、apeiron-cli 与 Chentu 的正式发行基础版本对齐为 `0.2.1`；CLI 更新不会自动升级已有部署。

- stable：标签 `v0.2.1`，版本必须与 Cargo 基础版本完全一致。
- nightly：标签 `v0.2.1-nightly.YYYYMMDD.RUN_ID`；日期使用北京时间，流水号使用 GitHub Actions run ID。同一基础版本下 nightly 排在 stable 之前。
- latest：与 stable 同步的默认别名，不是第三个通道。本轮暂不支持 beta/rc 发布。

三个仓库使用相同 nightly 规则；各仓库 run ID 独立，配套关系固定在发行清单与源码引用中。
CLI 始终锁定 Chentu 的版本、提交和 SHA-256；部署包锁定 Apeiron 镜像摘要，安装时不查询各自最新
nightly 拼装。正式发行需验证三者的兼容组合；nightly 不靠改名或取消 prerelease 标志晋级。
Apeiron 历史 `0.YYMMDD.N` 日期版本保留，转入 `0.2.1` 是编号规则迁移，不以旧标签的数字大小决定默认版本。

当前修复分支仍引用 Chentu `0.1.0-rc.5`，因此还不具备三者 `0.2.1` 正式发布条件；
必须先发布包含账户修复的 Chentu `0.2.1` 并更新固定引用，再完成整套安装与登录验收。
正式 tag 的构建配置会检查 Apeiron 子模块基础版本及 Chentu 固定版本，任一未对齐即拒绝构建。
nightly 允许验证尚未正式发行的固定组合，但不会因此被视为正式配套验收通过。

JS 发布工具从 Cargo 基础版本及标签解析实际版本，通过 `APEIRON_BUILD_VERSION` 注入 Rust 构建；本地无覆盖参数时使用基础版本。`--version`、`--build-info`、归档、provenance 和发行清单必须一致。nightly 不改写 Cargo 文件，仍要求源码提交与工作树干净。

`Nightly CLI` 工作流每天北京时间 03:00 调度（GitHub 调度可能延迟），也可在 main 手动运行。它读取最近成功发布的 nightly 清单；相同源码提交跳过，失败未发布的提交可重试。基础版本已经进入 stable 时暂停生成该版本的 nightly，维护者在正式发布后将 main 推进到下一目标（例如 `0.3.0`）。

调度器为固定提交创建不可变 nightly 标签，再以该标签 `workflow_dispatch` 触发 `Standalone CLI`。`GITHUB_TOKEN` 创建标签不会自动触发 tag push 工作流，因此必须显式 dispatch；无需额外 PAT，也无需将 OSS 信任范围扩展到 main。手动调度仅允许 main，发布工作流仅允许版本标签进入发布阶段。

### 正式发行步骤

1. 更新根 `Cargo.toml` 的 `workspace.package.version`（基础版本唯一来源）及 `Cargo.lock`。Chentu 更新时先发布并验证安装包，再同步更新 `runtime/src/app/resources.rs` 的版本、提交、URL 与 SHA-256。
2. 运行 README 中的 Rust、等价性和浏览器验证，提交推送代码。
3. 对待发布提交打相同版本标签并推送，例如 `git tag v0.2.1`、`git push origin v0.2.1`。不要求自动合并 PR。
4. `Standalone CLI` 工作流构建四个平台：macOS ARM64/x64、Linux ARM64/x64（musl 静态链接）。四个平台均在对应原生 runner 构建并测试；Linux 额外在 Ubuntu 22.04 容器内验证。另在隔离 Ubuntu 容器内，从非管理员 shell 验证权限不足会失败、sudo 安装后同一 shell 可直接运行、重复更新和文件所有者/权限。
5. 两个通道都必须通过完整检查和四个平台的独立启动、版本/构建信息、页面、favicon、配置 API 测试。发布 job 将相同产物保存到 OSS 与 GitHub Releases，逐文件下载校验后才推进通道。stable 更新 `channels/stable.txt`、公共 `install.sh`、`latest.txt`，nightly 仅更新 `channels/nightly.txt`。nightly 的 GitHub Release 标记为 prerelease，且不会成为 GitHub Latest。

所有标签的 publish job 使用同一个并发组串行执行，并在更新前重新读取通道、按版本数值比较；晚完成的旧版本只能归档，不能倒退指针或公共安装器。三个 stable 入口逐个更新，不能保证跨对象瞬时原子性；中途失败应重跑失败的 publish job，使用已验证的原始 artifacts 修复同版本入口，不能用重新构建的不同字节覆盖旧版本。所有已发布版本先永久保留，不设置自动清理。

独立 `release-contracts` job 不读取私有子模块或云凭据，验证版本、安装与发布行为；完整 Rust/四平台构建仍需 `ZYGOS_READ_TOKEN` 能读取全部固定子模块。凭据缺失或权限不足会阻止两个通道发布，不降低 nightly 的质量门槛。

手动构建当前平台（固定 Rust 工具链 + Bun 1.3.14，仅开发和发行脚本需要 Bun）：

```sh
git submodule update --init --recursive
bun run build:release /absolute/external/output
# 收集四个平台的构建产物后再生成汇总元数据：
bun scripts/finalize-release.js /absolute/external/output
```

构建要求已提交且干净的 Git 工作树，包括未追踪文件；Zygos 子模块必须已初始化、与 gitlink 一致且无本地改动（私有仓库 CI 权限见 [子模块说明](zygos-submodule.md)）；编译前后提交、tree、Cargo.lock 与工具链摘要必须保持一致。apeiron --build-info 显示编译时注入的确切提交、是否 clean、Rust 编译器和目标平台。每个归档含 build-provenance.json，对应 .provenance.json sidecar 还绑定归档摘要。封版逐个平台读取归档中的实际二进制并复核来源、工具链、target 与二进制/归档摘要；拒绝用当前 HEAD 重新标记旧产物。

每个发行目录包含四个归档、安装脚本、`SHA256SUMS` 和记录版本/源提交/构建器/文件摘要的 `release.json`。版本路径不可覆盖；网络失败重跑仅接受相同摘要，已有不同文件时停止。不要修改已发布版本，应增加新版本号。

OSS 地址结构：

```text
apeiron-cli/install.sh
apeiron-cli/latest.txt
apeiron-cli/channels/stable.txt
apeiron-cli/channels/nightly.txt
apeiron-cli/releases/<version>/apeiron-<version>-<os>-<arch>.tar.gz
apeiron-cli/releases/<version>/SHA256SUMS
apeiron-cli/releases/<version>/release.json
apeiron-cli/releases/<version>/install.sh
```

GitHub Actions 使用阿里云 OIDC，无长期 AccessKey。仓库变量为 `OSS_ROLE_ARN`、`OSS_OIDC_PROVIDER_ARN`；RAM 信任策略仅接受本仓库 `refs/tags/v*`，权限仅覆盖指定 bucket 的 `apeiron-cli/*` 对象。PR 构建没有云写入权限。IdP 证书轮换时，按阿里云流程更新经过验证的 CA 指纹。

RAM 信任策略的 Action 为 `sts:AssumeRole`，Principal 为该 OIDC provider。仓库启用了 immutable subject，`oidc:sub` 前缀应从 `gh api repos/TheApeironLab/apeiron-cli/actions/oidc/customization/sub` 读取，再追加 `:ref:refs/tags/v*`，不要只填写仓库名称。上传 job 显式设置 `ALIBABA_CLOUD_REGION_ID=cn-beijing`，不依赖开发机的阿里云 CLI 配置。

回退 CLI 可重新安装指定旧版本；平台数据库、集群或应用不会随 CLI 回退。

## Chentu registry 镜像供给

目标声明 `registryImages: 1` 的新包，在在线 `apeiron init` 完成发布清单校验后，
非引导且带固定 `registrySource` 的镜像由 Chentu supply 直接复制到现场 Nexus，CLI 不下载其归档。
普通文件、引导镜像与系统工具仍按原大小/SHA-256 校验。离线模式、旧包及无来源镜像使用完整归档。
镜像源错误不会回退为 tag、其他镜像站或公网 Pod 拉取；运行时仍只使用现场 Nexus。
该能力需要匹配的新 Chentu 包，不改写已发行清单，也不自动发布任何镜像。

### Chentu public preparation v1

A target declaring `publicPreparation: 1` requires Python 3 on the online operator.
Before cluster creation, `apeiron init` runs the Chentu package's standard-library
public preparation helper. It tries declared immutable public image/file sources,
then downloads the original OSS resource only when those sources are unavailable.
Image graph, target architecture and byte-integrity failures stop installation.
Bootstrap images participate; native K3s system archives and binaries can use
SHA-256-verified official downloads. A generated OCI archive updates only the local
staging plan's archive checksum, leaving the release catalog and image digest intact.
Offline runs never invoke the helper; older catalogs keep their existing behavior.
The new Chentu material must preserve upstream OCI manifests: old Docker-save-derived
digests may not exist publicly and will legitimately fall back to OSS.

Public preparation also recognizes `publicArtifacts` for fixed tar.gz/zip tool
members. The packaged Python helper bounds download/extraction and verifies the
original file SHA-256 and size. Unlike OCI repackaging, tool preparation cannot
change the local plan identity. Valid cached files skip the helper; offline
mode never invokes it. APT public URLs are publisher-bound to exact package bytes.

### 未发布：自有镜像按 OCI 对象准备

配套 chentu 的 `layerPreparation: 1` 将 `ociObjects` 引用绑定到 catalog 的
`imageObjects` 资源目录。仅选中组件的对象元数据进入准备请求，普通 files 仍保留
完整离线镜像归档；旧客户端不解析该附加目录，离线模式不启动公网准备程序。
新客户端按已验证缓存、公共 registry、单对象 OSS 准备完整 OCI 图，错误内容或
对象不可用停止，不回退下载整个镜像。发行合包、profile handoff 和 preview 都校验
并绑定独立对象的内容地址。旧发行资源不改写；该能力须随新 chentu 与 CLI 一同发布。
