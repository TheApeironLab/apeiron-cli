# 隔离升级入口（已完成限定副本验收）

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
| `status` | 同上 | 运行期间只读持久快照，不争用执行锁；不将 Helm deployed 当成 hook 成功 |
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

## 已完成的限定验收

2026-10-23，candidate-52（`0.1.0-dev.20261023.2`）通过包内统一入口在授权隔离
single-k3s 副本完成 `prepare → execute → verify / reconcile / resume → commit`。
CLI 源码 `8c0db97`，Chentu 源码 `459a6ba`；安装包构建没有读取部署集群。

- 错误 plan 摘要在变更前被拒绝；执行期间 `status` 成功返回 running 快照。
- IAM revision 5→6、Task 7→8；其余 26 个 release 不变。四个原生 hook 阶段、13 个细分步骤完成。
- 在已完成 sync 后，仅中断只读 `data-after` 验证器，再经精确对账摘要续执行。
  sync 回执及全部 Helm revision 不变，没有重放 Chart。未宣称真实注入过 native hook 中断。
- 47 个活动 Pod Ready、healthz 正常，26 个 PVC/PV 身份保留；6 条历史 Failed Pod 保留。
  11 个 PostgreSQL 库 / 578 张表没有旧行或列缺失；24 个 S3 桶 / 15 个对象及 Git refs/fsck 通过。
  心跳、租约、控制器观察、认证审计/会话及 Stalwart 内存子空间变化逐项审阅，没有使用数据丢失例外。
- 成功 sync 日志 ERROR=0；正式提交 generation 5，旧 generation 4 及资源/扩容信息保留。
  记录 SHA-256：`3755707b589265e49328a843d29ea549e1453227acd47cbb05c851cf0cb1cf93`。
- 归档 SHA-256：`425422f392d5381dc0608c762e00c1449cc05f2d55ad89c5cc6ad11b4301ab49`；
  验收 SHA-256：`6383c83db8c78d34b8a46738a27c07a797ff123e1632fc828d446a8af029d697`。
  私有证据目录 `unified-entry-2/` 保留计划、检查点、中断/续执行回执、数据指纹及最终资源清单。

本次是交付包升级和相同业务制品重同步，不能作为不同 Chart/镜像的迁移证明。
原机未修改；沿用已审历史恢复点，未生成新鲜备份，不声明 RPO=0、生产容量或完整业务验收。

## 组件范围与批次

符合契约的多个组件应放入同一份计划，由原生 Helmfile 按依赖处理；不是每个组件都必须
分批升级。当前正式适配器仅覆盖 IAM/Task，其他已演练组件接入统一入口及不同制品迁移
仍是实现缺口，不能静默跳过后宣称全组件升级完成。扩容须独立验收并更新基线，再升级应用；
应用操作不得自动撤销已验收扩容。

## 后续组件适配

联合 Chentu PR 增加了 11 个组件的原生 hook 检查点、包内冻结跨版本契约核验，
以及准入前的 `component-assessment.json`。该评估逐项列出未选更新、来源未知和未受管组件，
不能静默漏项后宣称全量完成。新增组件仍须具体版本路径与完整数据证据验收；
candidate-52 的已验收范围保持 IAM/Task 相同制品，生产执行仍阻断。


## 新副本升级前的保护范围核对

包内 Chentu 执行器在选择 Apeiron 的 `prepare` 中增加
`workspace-protection-scope.json`，输出到包外私有状态目录。该报告从实际集群读取
全部 Apeiron PVC，包含没有活动 Pod 的 workspace，并核对 PV/PVC UID、节点与本地路径。
远程卷、重叠路径、缺失 skills 卷和未分类临时数据会明确列为阻断，不按名称猜测可丢弃数据。
报告可能含现场路径，不能直接作为公开 PR 附件。

文件适配器可完整计算资源字节及用户扩展属性/POSIX ACL 指纹，冷归档与空目录恢复
保留这些属性；硬链接、嵌套挂载和不支持的安全属性拒绝。指纹不是备份，空目录文件恢复
不是应用恢复，报告中的 `executionAllowed` 保持 false。

完整资源发现、冻结窗口与匹配恢复集的执行集成，以及旧覆盖层来源和目标组合迁移证明，
仍须分别验收；Apeiron 的完整数据适配器守卫没有删除。必须用新副本自己的基线重新规划，
不能沿用历史副本记录。源码修改不自动进入已封装的 candidate-53；后续包以新版本发布。
此次流程在用户手工配合执行新副本升级前停住，生产操作仍须独立审阅批准。
