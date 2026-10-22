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

Apeiron 的 JavaScript 源码检查跳过确切的 `third_party/zygos` 路径，Bun 测试发现
限定为本仓库的 `test/`。Zygos monorepo 内的 TypeScript 与测试由上游独立验证，
不纳入宿主的语言限制或 Bun 测试；嵌入集成仍由 Apeiron 的 Rust 测试覆盖。

Cargo 会合并共享依赖的 features。Zygos 所需的 `serde_json/arbitrary_precision`
必须保留；Apeiron 输出 YAML 时先经 JSON 文本转换为原生 YAML 值，避免高精度
数字的私有 Serde 表示泄漏为配置对象。数值、嵌套值与字符串边界有回归测试。

`zygos_cli::run_with_command` 由 Zygos 提供，Apeiron 只负责子命令分发与运行时/取消。
最终产物仍是单个静态链接的 `apeiron`；运行者不需要 Git、子模块、独立 Zygos CLI、
Rust 或 Node。Zygos API 服务仍须单独部署。

发布检查拒绝未初始化、偏离 gitlink 或有本地改动的子模块；父仓库 tree 摘要已包含
固定 gitlink，因此发行来源绑定该 Zygos 提交。

## 私有仓库的 CI 权限

固定子模块 Zygos、Apeiron 和 Apeiron IAM 均为私有仓库，默认 GITHUB_TOKEN 只可读取当前仓库。
在 `apeiron-cli` 的 Actions secret 中配置 `ZYGOS_READ_TOKEN`（保留历史变量名），
使用资源所有者为 `TheApeironLab` 的 fine-grained token，仓库范围仅选择
`apeiron-cli`、`zygos`、`apeiron`、`apeiron-iam`，授予 Contents: read 和自动包含的 Metadata: read。
不需要 Actions、Workflows、写入或组织管理权限。令牌到期前需重新生成并更新同一个 secret。
工作流递归检出并关闭凭据持久化；凭据不得加入 Git URL、`.gitmodules` 或日志。
此代码修改不会创建、扩大或保存任何凭据权限。工作流在 secret 缺失时直接失败并说明
所需权限，不再回退到没有跨仓库访问权的 GITHUB_TOKEN。

当前 Zygos 仓库禁止 deploy keys（GitHub API 返回 `Deploy keys are disabled for this repository`）。
不要为此关闭仓库/组织安全策略，也不要复用开发者本机的个人登录 token。维护者需在
apeiron-cli Settings → Secrets and variables → Actions 中设置以上专用只读 secret；
完成后重新运行失败的 workflow。

没有该 secret 的 fork PR 无法检出私有子模块，需要有权限的受信任分支执行 CI；
不要通过 pull_request_target 执行未信任 PR 代码来绕过此边界。
