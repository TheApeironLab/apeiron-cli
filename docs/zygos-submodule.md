# Zygos 子模块

Zygos 的 Rust CLI 只在 Zygos 仓库维护。Apeiron 将整个仓库挂载为
`third_party/zygos` Git submodule，并在 `runtime/Cargo.toml` 通过 path dependency
引用 `third_party/zygos/apps/cli`。父仓库 gitlink 锁定确切提交，Cargo.lock 锁定 Rust 依赖。

## 构建与更新

```sh
git clone --recurse-submodules https://github.com/TheApeironLab/apeiron-cli.git
# 已有 checkout：
git submodule update --init --recursive
cargo build --locked --release
```

更新时先在 Zygos 仓库提交、验证并推送 CLI 改动，再在子模块中 fetch 并 checkout
经过审核的确切提交；提交 Apeiron 的 gitlink 和必要的 Cargo.lock 更新。
不要在构建中使用 `git submodule update --remote`，也不要复制 `apps/cli`。

`zygos_cli::run_with_command` 由 Zygos 提供，Apeiron 只负责子命令分发与运行时/取消。
最终产物仍是单个静态链接的 `apeiron`；运行者不需要 Git、子模块、独立 Zygos CLI、
Rust 或 Node。Zygos API 服务仍须单独部署。

发布检查拒绝未初始化、偏离 gitlink 或有本地改动的子模块；父仓库 tree 摘要已包含
固定 gitlink，因此发行来源绑定该 Zygos 提交。

## 私有仓库的 CI 权限

当前 Zygos 为私有仓库，默认 GITHUB_TOKEN 只可读取当前仓库。
需由维护者在 Apeiron 仓库/组织的 Actions secret 中配置 `ZYGOS_READ_TOKEN`，
使用对 Apeiron 与 Zygos 两个仓库仅有 Contents: read 权限的 fine-grained token。
工作流递归检出并关闭凭据持久化；凭据不得加入 Git URL、`.gitmodules` 或日志。
此代码修改不会创建、扩大或保存任何凭据权限。

没有该 secret 的 fork PR 无法检出私有子模块，需要有权限的受信任分支执行 CI；
不要通过 pull_request_target 执行未信任 PR 代码来绕过此边界。
