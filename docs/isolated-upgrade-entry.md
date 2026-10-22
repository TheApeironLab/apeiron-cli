# 隔离升级入口（开发中，尚未完成真实入口验收）

入口为 `bin/apeiron lifecycle upgrade`，固定调用同一安装包内的 Chentu 执行器。
不接受外部执行脚本，不转发插件，不通过首次安装向导接管已有集群。生产、扩容和恢复
仍阻断；`lifecycle plan` 的只读结果本身不授权执行。

包内容验证先于 Python 启动。CLI 绑定 SHA256SUMS、实际 CLI/Python 二进制和完整包内
Python 模块集合，拒绝未知导入文件；子进程不继承代理、Python 路径和动态加载器变量。

所有操作需要：

```text
--package /external/apeiron-install-VERSION-linux-amd64
--package-sha256 SHA256_OF_SHA256SUMS
--python /external/operator/venv/bin/python3
--python-sha256 SHA256_OF_PYTHON_EXECUTABLE
--state /external/operation
```

| 操作 | 附加参数 | 行为 |
|---|---|---|
| `prepare` | `--request FILE` | 核对副本、旧记录、逐组件基线、保护证据并只读渲染，生成待审计划 |
| `execute` | `--plan FILE --plan-sha256 SHA256` | 准入 → 前指纹 → 原生 Helmfile → 后指纹 → 待审验收 |
| `verify` | 同上 | 只读刷新后指纹与验收，保留历史证据和已完成 sync |
| `status` | 同上 | 读取持久检查点；不将 Helm deployed 当成 hook 成功 |
| `reconcile` | 同上 | 核对实际状态并输出精确对账摘要 |
| `resume` | 同上，另加 `--reconciliation-sha256 SHA256` | 仅执行已证明可继续的阶段，不自动重装 Chart |
| `commit` | 同上，另加 `--review-sha256 SHA256` | 无硬阻断且审阅摘要匹配时提交新 generation，保留旧记录 |

现场 request、plan、状态和日志必须在包外私有目录。安装包构建不读取任何部署集群。
当前执行适配器限 single-k3s 的 IAM/Task、且应用制品与已验收记录完全一致；新交付包
可重同步这些应用，但这不能冒充不同应用版本的迁移测试。改变 Chart/镜像时缺少受支持
迁移适配器会阻断。

数据检查保存 PostgreSQL 行/列指纹、S3 全量对象摘要、Git refs/HEAD 与 fsck 结果，
并核对 PVC/PV 身份及未选中 release。缺失数据、未知配置变化或运行镜像不符不会因
传入 review 摘要而豁免。历史恢复点例外只限明确审阅的副本演练，不声称新备份或 RPO=0。

续执行只在所有选中 Chart 到达精确的新 revision，且 native prepare 回执完整时续跑
IAM/Task finalize；已完成配置跳过，失败证据保留。部分 Chart 未完成仍明确阻断。
现场漂移、未支持迁移、未验收组件和生产目标均不能靠更换 state 目录绕过准入。
