# 统一发布编排

统一入口位于 `apeiron-cli`。Apeiron 提供应用源码和镜像配方，Chentu 提供部署源码和官方 publisher，
CLI 仓库固定交付组合、构建四平台二进制、执行安装验收并更新下载通道。发布不调用生产部署工作流。

## 从源码到下载通道

1. `release/recipe.json` 固定 Apeiron、Chentu、System Skills 的完整已合并提交、第三方版本、
   两种架构的基础材料摘要和发布签名身份。`Prepare unified distribution` 仅从 `main` 运行，
   首先检查源码提交属于各自的 `main` 历史。
2. 全局版本只生成一次。stable 使用 Cargo 基础版本；nightly 使用
   `X.Y.Z-nightly.YYYYMMDD.RUN_ID`，日期取原 workflow 创建时间的北京时间。
   重跑同一 workflow 保留版本、源码提交和签名时间。
3. 构建 backend/app/ops/scode 的 AMD64、ARM64 镜像，以及 macOS ARM64、macOS Intel、Linux AMD64
   接收端。所有 Apeiron 组件使用同一全局版本；Sudocode 保持独立的上游版本。
   镜像实际执行版本和前端运行配置检查。System Skills 仅生成一个共享的 Git bundle。
4. 按公开 SHA-256 地址下载平台基础材料，逐文件验证内容、大小和执行权限；部署源码重新从固定 Chentu
   提交生成。使用 Chentu 原有 Helmfile 图、材料检查、Debian 包闭包检查和官方 publisher 生成两个 profile。
   原生 sandbox APT 仓库及最终部署归档使用保护环境中的签名密钥。
5. 两个 profile 的部署源码、版本、来源和公共材料必须一致，才组合为 schema 2 部署目录。
   每个 profile 先上传并回读验证资源，再交付保留原始校验清单的轻量 artifact，避免将整套镜像
   打包、下载、解压多次。组合步骤按目录中的大小和摘要完整读取公开资源；两种架构共享的对象只验证一次。
   归档和签名按固定输入生成；每个对象上传时使用 `forbid-overwrite`，并完整回读公开 URL 核对字节。
   同名对象内容不同则失败，不能覆盖已发布版本。
6. 自动创建 `codex/distribution-<version>` PR，只修改 Apeiron gitlink、编译进 CLI 的 Chentu
   VERSION/REVISION/DIGEST、`release/distribution-input.json`。PR 构建读取该固定版本，执行完整检查和四平台构建。
   此时上游制品已可下载，但 CLI 下载通道还没有更新。
7. PR 审核并合并后，nightly 调度器给这个固定组合创建对应版本标签并启动 `Standalone CLI`。
   stable 必须由发布负责人明确授权创建 stable 标签；合并源码或准备 stable 制品不等于发布 stable。
8. 标签构建成功后，`Unified release` 核对原构建的仓库、工作流、提交和成功状态，下载原产物，
   验证公开 Chentu 归档摘要和签名，执行四种目标的真实安装。
9. 四个验收全部成功，才生成与构建、发行输入和本次验收 run 绑定的 `distribution.json` 并发布。
   stable 更新 stable/latest/install.sh；nightly 只更新 nightly。

构建部署包所需的 publisher CLI 与最终交付 CLI 分开：前者来自编排源码提交，包含当时的旧安装引用，
仅用作 Chentu 构建工具，其实际二进制来源保留在 provenance；后者在固定新部署包的 PR 合并后构建。
这避免“先有最终 CLI 才能构建它所依赖的部署包”的循环依赖。

## 基础材料与来源

`freeze-platform.py` 将经过验证的基础材料转换为按内容寻址的 seed。它不携带 Apeiron 应用镜像、
接收端、System Skills、Chentu 源码或旧 sandbox 签名；这些由准备工作流从固定来源重新生成。
修改 seed 必须重新核对材料闭包、上传不可变内容并通过配方 PR 更新摘要。

公开包的应用范围由 `runtime/assets/apps.json` 决定，`applicationScope: wizard` 包含全部
可选应用及其依赖。准备入口先检查 seed 中的组件与资源；构建 profile 和组合归档时再检查一次。
`requiredComponents` 只列基础根组件，不能用它缩减向导承诺的应用范围。ARM64 profile 出包后使用
真实 Rust 规划器、Helmfile 和 check.py 验证逐个可选应用及全部应用组合，保留基础、Task、Langfuse、
Grafana 的原有断言。检查容器没有网络，不把渲染成功视为运行验收通过。

首次补齐平台材料时，`Prepare optional application images` 从 `release/platform-images.json`
指定的已合并源码和正式 Dockerfile 构建 Zygos API/Web/Worker/EvalScope 与 Browser 的 AMD64、ARM64
镜像。工作流仅保留镜像归档、源提交、配方摘要和归档摘要；不发布通道，也不自动替换平台 seed。
同一工作流通过临时 `GITHUB_TOKEN` 的 `packages: read` 获取固定 digest 的 GPUStack
多架构 OCI 归档。该私有包须授予 `apeiron-cli` Actions 的 Read 权限；不需要增加长期 PAT。
导出后验证原始 index digest、全部 blob 摘要和引用闭包，以及 AMD64/ARM64 的存在；截断归档、
缺失层和只有单一架构的导出均拒绝。临时认证文件在 job 结束前删除，artifact 不包含凭据。
这些镜像与其他应用材料通过 Chentu publisher 的闭包验证后，才能冻结并提交新的 seed 摘要。
SFTPGo 的旧多架构 pin 只在本地构建过，GHCR 没有对应对象。`Prepare SFTPGo source images`
从 `sourceBuilds.sftpgo` 固定的公开 PKCE 源码提交、Dockerfile 摘要和 Go 版本重建两种架构，
分别在原生 runner 执行版本检查，记录二进制、CA、镜像和归档摘要。它也由完整平台材料工作流调用，
可单独运行以补齐材料。构建只产生 Actions artifacts，不写 GHCR 或下载通道；将两种架构组合为
固定 OCI index 后，须同步更新 Chentu 的镜像与对应源码声明，通过完整材料检查后再冻结 seed。
当前基础 seed 尚未包含全量应用，准备入口会拒绝它；不得为了继续发布而删减向导范围或跳过检查。

GPUStack 沿用 Chentu 的独立 GPU 主机契约：向导收集 GPU 主机 IP 和服务端口，部署集群中的
Service/Endpoints/Ingress。GPU 驱动、主机服务、模型权重由 GPU 主机单独准备。配置与镜像检查、
实际入口/SSO 验收、模型推理验收分别记录，CPU 安装测试不能代替 GPU 推理测试。

`release/signing-key.asc` 仅包含公钥。正式私钥不进入 Git；`recipe.json` 固定其完整指纹。
Chentu 归档旁保留 `.asc` 和 `provenance.json`，后者记录原始候选及两种 profile 的完整来源和构建工具摘要。
同版本重试应复用成功的 Actions artifacts，只重跑失败 job；重新构建得到不同字节不能覆盖同版本对象。

发行输入绑定三者的同一版本、Apeiron/Chentu 完整提交、Chentu 公开归档摘要和 URL，
以及四种应用镜像的 AMD64、ARM64 manifest digest。digest 必须是实际运行的架构 manifest，
不能使用浮动 tag 或多架构 index digest。缺少真实部署包时不得填占位摘要或虚构输入。

## nightly 调度

`Unified nightly` 先检查公开 stable 指针。Cargo 基础版本不高于 stable 时停止，要求先推进开发版本。
有待审核的 distribution PR 时保留该候选；已合并但尚无标签时验证引用再创建原 nightly 标签。
若标签已有构建，提示检查或重跑原 workflow，不为同版本创建另一份验收身份。
当 `main` 已推进且没有待审核候选时，准备新的整套上游制品。

分支仍可手动运行 `Standalone CLI` 并填写与 Cargo 基础版本一致的 `candidate_version`，
获得未发布的四平台构建用于诊断。该入口保留完整检查，不会触发下载通道发布。

## 权限配置

| 环境或凭据 | 允许来源 | 用途 |
| --- | --- | --- |
| `ZYGOS_READ_TOKEN` | GitHub 构建 | 只读访问 CLI、Apeiron、IAM、Zygos、Chentu、Skills |
| `deployment-build` | 仅 `main` | `CHENTU_SIGNING_KEY` secret、指纹变量；OIDC 仅写 Chentu 版本目录和内容寻址 blob |
| `deployment-pins` | 仅 `main` | `DISTRIBUTION_PR_TOKEN`，仅 CLI 的 Contents/PR 写权限，创建普通 PR 事件并触发 CI |
| `deployment-acceptance` | 仅 `v*` 标签 | `ACCEPTANCE_ECS_ROLE_ARN` 和固定测试网络/镜像配置；OIDC 管理有验收标签的临时 ECS |
| CLI 下载发布角色 | 版本标签 | 仅统一发布最后一步写 CLI 发行对象与通道 |

组织当前禁止 `GITHUB_TOKEN` 创建 PR，因此使用独立的 CLI 专用 PR 凭据，不修改组织策略。
所有有限期凭据需在到期前轮换。验收角色没有生产部署权限；创建实例限定测试 VPC、实例规格、按量计费和用途标签。
基础材料上传角色不能写 CLI 通道；发布角色的现有范围不能替代部署材料权限。
超过 5 GiB 的内容寻址 blob 使用分片上传，逐片核对 ETag，完成时仍由 OSS 原子拒绝覆盖已有对象，
成功后完整回读公开内容。失败时仅中止本次创建的 upload ID；构建角色的 `oss:AbortMultipartUpload`
仅授予 `apeiron-bj-cli-downloads/apeiron/blobs/sha256/*`，不包含删除已完成对象的权限。

## 四种验收目标

| 目标 | 执行环境 |
| --- | --- |
| `k3d-darwin-arm64` | macOS ARM64 自托管 runner、Docker、临时独立集群 |
| `k3d-linux-arm64` | Linux ARM64 自托管 runner、Docker、临时独立集群 |
| `k3s-single-amd64` | 临时 Ubuntu 22.04 AMD64 ECS，一台 server |
| `k3s-multi-amd64` | 临时 Ubuntu 22.04 AMD64 ECS，三台 server，HA 开启 |

两个 ARM runner 放入 `apeiron-release` 专用 runner group，仅允许 CLI 仓库的
`.github/workflows/release-acceptance.yml@refs/heads/main` 调用，并各自带对应目标标签。
该工作流须先存在于 `main`，才能配置 GitHub 的工作流限制。runner 设置
`APEIRON_RELEASE_DISPOSABLE=true`；测试脚本生成独占配置路径、域名和端口，最终只清理本次集群的容器、网络和卷。
禁止使用 Docker prune。安装配置和原始日志仅写入私有临时目录（runner 完成 job 后可能清理），只上传有界验收结果。

原生测试在 GitHub hosted Ubuntu runner 上编排，通过云助手运行，无需持久 GitHub runner 注册令牌或公网 SSH 放行。
测试机固定为 8 核 32 GB、200 GB 磁盘，按量计费并设置 4 小时自动释放。
单机和三机测试串行执行，同时最多三台。每次生成独立 SSH key，仅在测试 VPC 内连接节点。
传输包放私有 OSS 路径，经临时签名 URL 下载并核对摘要；密钥、配置和安装日志不进入公开制品。

工作流在成功、失败后均尝试删除本次带完整用途/运行标签的实例、密钥对及传输包，并确认实例已消失。
清理前检查实例身份、规格、网络、镜像和到期时间；不匹配则停止清理并报错。
强制中断使清理无法执行时，自动到期作为兜底。云助手失败诊断最多保留 12 KB，位于
`apeiron/acceptance-staging/<lease>/diagnostic.txt` 私有对象；由维护者排错后清理，不公开上传 Actions artifacts。

所有环境和权限就绪后才设置仓库变量 `RELEASE_ACCEPTANCE_READY=true`。该开关只表示基础设施可用，
不代表某个版本已经通过安装验收。标签必须属于 reviewed `main` 历史，PR 不能执行验收 runner。

## 验收内容与边界

每个目标使用最终 CLI 编译引用的公开 Chentu 包安装，核对节点数量、角色、Ready、架构和原生 Ubuntu 版本，
检查四种应用运行镜像摘要、持久 Keycloak operator 凭据、CA 校验的 HTTPS，以及真实 IAM 密码 token 登录。
SSO 配置重启 K3s 后会等待服务恢复。scode 按需启动，因此创建临时 Pod 检查镜像和可执行文件版本，完成后删除。

上述基线不包含浏览器 PKCE/会话、模型调用、应用或存储故障切换、可选组件及完全断网验收。
三 server 的基础安装通过不等于完整 HA。使用私有开发部署包的旧测试不能替代公开固定引用的发行验收。
`scripts/smoke-native-host.py` 的主机启动检查也不算安装验收。

## 重试与发布状态

未开始上传最终发行文件时，可以在同一标签调用 `Unified release` 并指定原成功 build run ID。
一旦写入最终不可变发行文件，必须重跑原 Unified release 的失败 job，保留原 run ID 和验收记录，
不能重新 dispatch 后覆盖同版本 `distribution.json`。固定引用 PR 若已创建，准备脚本会报告现有 PR；
若推送分支后创建 PR 失败，维护者应从该固定分支恢复 PR，不强推改写已生成的候选。

当前实现补齐了上游准备、签名、不可变上传、固定引用 PR 和临时 ECS 验收代码。
首次主分支完整运行、受限 ARM runner 注册及四目标公开下载安装仍须以真实 CI 结果验收。
在这些结果产生前，不启用发布就绪变量，不声称正式 `0.2.1` 已可用。
