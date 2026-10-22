# 统一发布编排

统一发布入口属于 `apeiron-cli`。Apeiron 仓库拥有应用镜像构建，Chentu 仓库拥有部署包构建；
CLI 仓库负责固定交付组合、四平台二进制、安装验收和下载通道发布。发布不调用生产部署工作流。

## 当前实现边界

`Unified release` 接管已构建 CLI 的验收与发布：

1. `Standalone CLI` 保留已有完整检查及四平台构建，版本标签构建完成后 dispatch 统一入口。
2. 统一入口核对原构建的仓库、工作流、完整提交、成功状态，拒绝 PR 构建；下载原产物，不重构建。
3. `release/distribution-input.json` 必须在该标签的源码中，绑定 Apeiron 提交、两种架构的应用镜像 digest、
   Chentu 版本/提交/归档 digest。其版本必须与 CLI 完全一致，nightly 也不例外。
4. 分别在 macOS ARM64 K3d、Linux ARM64 K3d、Ubuntu 22.04 AMD64 单机 K3s、三 server K3s 上安装。
5. 验收同时检查节点数量/Ready、运行镜像摘要、持久 Keycloak operator 凭据、CA 校验的 HTTPS，
   以及实际 IAM 密码登录。这里的登录检查是 IAM token 请求，**不替代浏览器 PKCE/会话端到端测试**。
6. 四个结果全部成功才生成 `distribution.json`，发布脚本再次核对结果与本次 workflow run、CLI 归档及发行输入摘要。
7. 复用已有不可变 OSS/GitHub 上传、下载回读、串行发布和防止版本回退逻辑。
   stable 更新 stable/latest/install.sh；nightly 只更新 nightly。

手动运行 `Standalone CLI` 并设置 `candidate=true`、Apeiron 和 Chentu 的完整提交，
会调用 `candidate-images.yml` 构建 backend/app/ops/scode 的 AMD64、ARM64 镜像。
`prepare-candidate.js` 根据原始 workflow 创建时间与 run ID 生成唯一 nightly 版本；
重跑沿用该版本。产物包含源码、配方、镜像与归档摘要，并保存在 Actions artifacts。
backend 和 scode 实际执行版本检查；app/ops 启动容器，检查两组站点 OIDC 参数、
运行配置响应和未被改写的前端资源。该入口只产生候选镜像，不发布下载通道。

**尚未接通自动构建/上传 Chentu 包及自动创建固定引用 PR。**
ARM64/AMD64 材料闭包，以及 CLI 构建工具与最终 CLI 的来源分离仍须接通；
不能把本工作流称为“三仓库从源码一键构建”。目前也没有可用的正式 `0.2.1` 原生部署包，
因此不提交虚假的 `distribution-input.json`，不启用环境就绪变量，不创建正式标签或触发发布。

## 准备发行输入

先固定三个仓库的提交和一个全局版本。nightly 只生成一次 `X.Y.Z-nightly.YYYYMMDD.RUN_ID`，
所有组件使用该版本；日期为北京时间。构建工具所用 CLI 可以是较早的已验证提交，
必须在 Chentu provenance 中记录它，不能将它冒充最终交付 CLI。

上游制品实际构建、验证并上传不可变地址后，更新 CLI 的 Apeiron gitlink 和
`runtime/src/app/resources.rs` 中 Chentu VERSION/REVISION/DIGEST。随后生成并评审发行输入：

```json
{
  "schemaVersion": 1,
  "version": "0.2.1",
  "apeiron": {
    "version": "0.2.1",
    "revision": "完整的40位提交",
    "images": {
      "backend": {"amd64": "仓库@sha256:摘要", "arm64": "仓库@sha256:摘要"},
      "app": {"amd64": "仓库@sha256:摘要", "arm64": "仓库@sha256:摘要"},
      "ops": {"amd64": "仓库@sha256:摘要", "arm64": "仓库@sha256:摘要"},
      "scode": {"amd64": "仓库@sha256:摘要", "arm64": "仓库@sha256:摘要"}
    }
  },
  "chentu": {
    "version": "0.2.1",
    "revision": "完整的40位提交",
    "sha256": "实际归档的64位摘要",
    "url": "https://downloads.apeironlab.cn/chentu/releases/0.2.1/chentu-0.2.1.tar.gz"
  }
}
```

以上是格式说明，不能直接作为发行输入。镜像 digest 是各架构实际运行的 manifest digest，
不是可移动 tag 或多架构 index digest。构建和验收必须使用对应的实际值。

现有 nightly 调度器仍只调度 CLI 构建，尚未生成整套新 nightly 上游制品及输入。
在该准备阶段接通前，它会被统一入口阻塞，不能回退为单独发布 CLI。

## 专用验收环境

四个自托管 runner 都带 `apeiron-release` 标签，另分别带：

| 标签 | 环境 |
| --- | --- |
| `k3d-darwin-arm64` | macOS ARM64、Docker、独立空集群 |
| `k3d-linux-arm64` | Linux ARM64、Docker、独立空集群 |
| `k3s-single-amd64` | 一台全新 Ubuntu 22.04 AMD64 |
| `k3s-multi-amd64` | Ubuntu 22.04 AMD64 操作机及三台独立 server；HA 开启，无 agent |

每个 runner 设置 `APEIRON_RELEASE_DISPOSABLE=true` 和 `APEIRON_ACCEPTANCE_CONFIG=/绝对路径/配置.json`。
该配置是向导配置请求正文，包含 apps、models、slug、deployment 等；不包含 revision。
必须选择 Apeiron、完整依赖、目标 topology 和全新站点域名，`deployment.offline=false`；
不允许覆盖 deployment.root/environment 绕开编译进 CLI 的包。
模型密钥、SSH 私钥、节点 inventory 留在受限 runner 文件中，不入 Git、公开发行文件或 Actions artifacts。

安装器会修改专用测试机/测试集群。只对已明确用于验收、可重建的机器配置这些变量。
环境管理员负责销毁/重建集群和恢复空白节点；工作流不自动卸载已有集群。
每个目标串行运行，失败保留私有安装目录供排错。重跑之前须恢复该目标为空白环境。
全部环境就绪后才设置仓库变量 `RELEASE_ACCEPTANCE_READY=true`。

云端单机的启动检查可运行 `scripts/smoke-native-host.py`，它验证系统、二进制来源、版本和向导 API，
输出 `kind=native-host-smoke`、`installationTested=false`，**不能作为任何安装验收结果**。

## 重试与权限

尚未上传任何发行文件时，统一入口也可在同一版本标签手动运行，输入原 `Standalone CLI` 的 build run ID。
一旦开始上传不可变发行文件，必须重跑原 Unified release 的失败 job，保留同一个 run ID 和原始验收记录；
不能重新 dispatch 后用另一次验收记录覆盖同版本的 `distribution.json`。
缺少任何制品、目标环境或验收结果均阻止发布。只有统一发布 job 获得 OSS OIDC 写权限；
仍使用本仓库版本标签的信任范围。上游制品上传需要另行配置 `chentu/*` 和 `apeiron/blobs/*` 权限，
现有 CLI 的 `apeiron-cli/*` 权限不能替代它们。

全套 real E2E、原生安装、多节点和上游构建尚未在本改动上完成。
本地合同测试通过只能证明发布门禁的拒绝/绑定规则，不能据此把 `0.2.1` 宣布为可发布。
