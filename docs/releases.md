# 发布与安装

主分发渠道为公开 OSS；GitHub Releases 保存相同文件和发行记录。GitHub 仓库仍为私有，用户从 OSS 安装无需 GitHub 账号。

## 用户安装与更新

```sh
curl -fsSL https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com/apeiron-cli/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
apeiron init
```

默认安装到 `~/.local/bin/apeiron`。安装器为 zsh/bash/sh 的启动文件添加 PATH；上面的 export 让当前终端立即生效。也可直接运行 `~/.local/bin/apeiron init`。向导在本机随机端口监听并自动打开默认浏览器。

再次运行安装命令更新 CLI；不会修改安装配置、创建集群或升级已经部署的应用。指定版本和目录：

```sh
curl -fsSL https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com/apeiron-cli/install.sh -o /tmp/apeiron-install.sh
sh /tmp/apeiron-install.sh --version 0.1.0-rc.1 --install-dir "$HOME/.local/bin"
```

安装器自动识别 macOS/Linux、ARM64/x64，下载对应 tar.gz，校验 SHA-256，再验证新二进制能够运行且版本一致，最后替换旧文件。下载或校验失败保留旧安装。`--no-modify-path` 禁止修改 shell 配置，`APEIRON_DOWNLOAD_BASE` 可指定具有同样目录结构的 HTTPS 内网镜像。

当前是预览版，`latest.txt` 指向最新通过验证的预览发行版。macOS 文件尚未做 Developer ID 签名和公证；浏览器下载后如被 Gatekeeper 拦截，应在系统设置中核实来源后允许运行，不要求全局关闭 Gatekeeper。

## 远程或离线

服务器无桌面时，在服务器运行：

```sh
apeiron init --no-open --port 3210
```

本机建立 SSH 转发 `ssh -L 3210:127.0.0.1:3210 user@server`，再用本机浏览器打开服务器打印的完整 setup URL。页面操作的文件、Docker 和系统配置属于运行 CLI 的服务器。

离线安装：从联网机器下载对应的 `apeiron-<version>-<os>-<arch>.tar.gz` 与同目录的 `SHA256SUMS`，拷贝到内网。用 `sha256sum`（Linux）或 `shasum -a 256`（macOS）核对该归档，再解压，将 `apeiron` 放入 PATH。运行 CLI 不需要 Bun/Node.js。

CLI 二进制不包含平台镜像。离线部署还需要对应目标的完整 Chentu 包：`chentu/` 部署程序、所选应用及依赖的镜像、Chart 和工具。向导选择“离线部署”并填写 CLI 主机上的包目录。当前 K3d 准备脚本仍需联网，CLI 明确拒绝 K3d 离线部署；内网平台安装需另行准备并验收原生 K3s 离线包。

当前 Chentu rc.5 提供 K3d ARM64 安装目标；原生 Ubuntu、多节点和 AMD64 需要另外发布经过验收的部署包。K3d 需要 Docker，其他应用转发命令需要单独配置相应 CLI。

## 维护者发布

1. 更新 `package.json` 的 version。Chentu 更新时先发布并验证安装包，再更新 `src/resources/chentu.ts` 的版本、提交、URL 与 SHA-256。
2. 运行 `bun install --frozen-lockfile`、`bun run typecheck`、`bun test`，提交推送代码。
3. 对待发布提交打相同版本标签并推送，例如 `git tag v0.1.0-rc.1`、`git push origin v0.1.0-rc.1`。不要求自动合并 PR。
4. `Standalone CLI` 工作流构建四个平台：macOS ARM64/x64、Linux ARM64/x64（glibc）。Linux x64 使用 baseline CPU 构建，Linux ARM64 在 QEMU 下测试，macOS 两种架构在对应原生 runner 测试。
5. 所有平台必须通过无 Bun 的独立启动、页面、favicon、配置 API 测试。发布 job 将相同产物保存到 OSS 与 GitHub Releases，逐文件下载校验后才更新 `install.sh` 和 `latest.txt`。

手动构建（Bun 1.3.14）：

```sh
bun run build:release /absolute/external/output
bun scripts/finalize-release.ts /absolute/external/output
```

每个发行目录包含四个归档、安装脚本、`SHA256SUMS` 和记录版本/源提交/构建器/文件摘要的 `release.json`。版本路径不可覆盖；网络失败重跑仅接受相同摘要，已有不同文件时停止。不要修改已发布版本，应增加新版本号。

OSS 地址结构：

```text
apeiron-cli/install.sh
apeiron-cli/latest.txt
apeiron-cli/releases/<version>/apeiron-<version>-<os>-<arch>.tar.gz
apeiron-cli/releases/<version>/SHA256SUMS
apeiron-cli/releases/<version>/release.json
apeiron-cli/releases/<version>/install.sh
```

GitHub Actions 使用阿里云 OIDC，无长期 AccessKey。仓库变量为 `OSS_ROLE_ARN`、`OSS_OIDC_PROVIDER_ARN`；RAM 信任策略仅接受本仓库 `refs/tags/v*`，权限仅覆盖指定 bucket 的 `apeiron-cli/*` 对象。PR 构建没有云写入权限。IdP 证书轮换时，按阿里云流程更新经过验证的 CA 指纹。

回退 CLI 可重新安装指定旧版本；平台数据库、集群或应用不会随 CLI 回退。
