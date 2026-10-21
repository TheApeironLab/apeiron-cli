# 生命周期契约与现场只读规划 v1

实现基于 PR #21 的 `f4df37d` 两份设计。首批范围为发布契约、安装记录/观测/备份引用契约及只读规划；**不是升级、扩容或备份恢复执行器**，不采集生产集群，不写入生产安装记录，不触发 Helm/Ansible/备份脚本。

## 使用入口

```bash
apeiron lifecycle validate-manifest --manifest /external/release-manifest.json
apeiron lifecycle validate-record --installation /external/installation.json
apeiron lifecycle plan --installation /external/installation.json \
  --observation /external/observation.json --intent /external/intent.json \
  --manifest /external/release-manifest.json --out /external/new-plan.json
```

应用恢复额外传 `--backup /external/backup-manifest.json`，`--manifest` 指向备份来源版本的清单。独立备份也使用来源版本清单，不要求存在目标升级版本。单文件最多 4 MiB，拒绝符号链接输入；输出新建且权限 0600，不覆盖已有文件。没有 `--out` 时输出计划 JSON，不包含 data resource 的 locatorRef 或配置/Secret 实值。

`planningStatus=ready` 只说明快照内部满足已实现的规划检查。输出永远 `readOnly=true`、`executionAllowed=false`，执行器、实时复检和真实验收仍是执行阻断项。`validate-record` 的 valid 只代表格式/引用合法，`liveVerified=false`；手工填入 evidence 或 healthy 不能代替现场采集和验收。

未实现的 lifecycle upgrade/expand/backup/restore/execute 命令明确失败，不转发到插件。向导显式携带生命周期 intent/planId 的执行请求也返回 409，不能被解释成旧首装请求。

## 发布与现场分离

`validate-manifest` 只校验生命周期元数据，不替代完整发行清单、制品或发布来源校验。

chentu 的 publisher 从固定源码和制品构建通用发布清单，不输入安装记录、节点、kubeconfig、已选组件或备份落点。`lifecycle` 只包含组件契约、备份适配器、迁移规则及逐组合验收的来源路径；默认 schema 未知且无允许跨度，不能把缺省值当作支持。冻结验收报告核验器尚未实现，publisher 当前拒绝发布非空允许跨度或 verified 适配器，不能手填摘要绕过。声明契约见配套 chentu 的 `docs/stack/lifecycle-planning.md`。

现场文件是另外四类对象。全部使用 camelCase、schemaVersion=1，拒绝未知字段；字段完整定义见 `runtime/src/app/lifecycle/model.rs`，不会宽松忽略拼写错误。集合字段须显式提供；estimatedBytes、backupAdapter 和 protection 可省略或为 null，相应信息缺失会在规划中阻断。集群/安装/节点身份禁止包含斜杠；所有标识拒绝空路径段及 `.`/`..` 段。标识为最长 128 字符的字母数字及 `._/-`，摘要为 64 位小写 SHA-256。字段只保存非敏感身份/摘要/引用；Secret 值不能填入这些文档。

| 对象 | 必需字段 |
|---|---|
| 安装记录 | kind=`chentu-installation-record`，installationId、clusterUid、generation、release、topology、architecture、nodes、configurationSha256、foundationSha256、components、resources、baselineEvidence、acceptedExpansions |
| 只读观测 | installationId、clusterUid、generation、recordSha256、configurationSha256、foundationSha256、nodes、components、healthy、drift、network（offline/online） |
| 操作意图 | operation（application-upgrade/backup/application-restore/expand）、installationId、clusterUid、expectedGeneration、selectedComponents、addAgents、protection |
| 备份引用 | backupId、installationId、clusterUid、generation、foundationSha256、topology、nodes、componentIds、resourceIds、level、manifestSha256、independentCopyRef、recipientFingerprint、restoreEvidence、sourceRelease、componentStates、resourceIdentities、configurationSha256 |

嵌套契约：

- release/sourceRelease：version、manifestSha256。发布文件摘要由 CLI 对**精确文件字节**计算，来源清单引用同样采用文件摘要。
- nodes：每项 id、role（server/agent）；不接受重复节点。single 拓扑须恰有一个 server；multi 保留实际 server/agent 集合，不推断 HA。
- components：每项 status（installed/absent/unknown）、version、state、configurationSha256、dataBindings。state 含 artifactDigests、configurationSchema、dataSchema。制品字节摘要映射不能代替未来对运行镜像/Chart 的真实核验；源端采集器尚未实现。unknown 不能解释为 absent。
- resources：每项 kind、owner、consumers、locatorRef、estimatedBytes（可 null）、backupAdapter（可 null）、restoreGranularity。组件 dataBindings 将稳定数据角色绑定到资源 ID；资源按 ID 去重。owner/consumer 必须属于已安装组件。每个被保护组件须有匹配的数据归属，缺适配器/数据角色/估算则阻断。
- protection：owner、rpoSeconds、rtoSeconds、downtimeSeconds、independentCopyRef、recipientFingerprint、capacityBytes。预算与独立副本是现场意图，规划不将其视为已验证的取回或解密结果。数据字节合计只是下界，解密/演练/新旧卷共存峰值仍须后续实测。
- acceptedExpansions：每项 id、generation、nodeIds、evidence。generation 不超过当前记录；节点须已在当前基线且角色为 agent。它是已有验收记录的引用，不是本批扩容验收器。
- 备份 level：generated/integrity-verified/isolated-restore-verified/disaster-restore-verified；现有备份产物仍须由未来校验器实际取回/解密/恢复，不能靠改 level 放行。

recordSha256 是安装记录 JSON 值的 `sorted-json-utf8-v1` 摘要：对象键递归排序、无多余空白、UTF-8 字符不转义、整数保持整数；可直接使用 validate-record 的输出。resourceIdentities 对每个来源 resource 对象使用相同算法。它们不同于发布文件原始字节摘要。planId 绑定全部输入以及规划结果；集群、基线、目标、配置、预算或操作意图变化都会生成不同身份。状态命名空间使用 clusterUid/installationId/planId。

## 规划边界与阻断

升级按实际安装集合匹配已验收的完整组件组合和来源清单；逐组件核对制品、schema、迁移规则及共享依赖状态。包中额外组件不自动安装；目标缺组件、新依赖、组件增删、未知 schema、基础组件变化或不明确路径均阻断。无变化的共享数据服务仍作为数据 owner/consumer 进入保护与冻结范围。只校验 Helmfile 清单中的依赖，不建立第二套安装排序或 values 合并器；原配置摘要保持不变。

内外网集群独立规划并绑定明确身份、generation、组件状态、节点和配置；观测与记录不符则阻断。相同包可分别规划 multi-k3s 与 single-k3s，也可以使用不同目标包；一个集群的状态和备份不能用于另一个。

有 addAgents 的应用升级计划先列出独立扩容、提交基线、应用阶段；扩容执行器未实现，应用阶段要求扩容后的新观测/基线和新鲜应用备份。内网 3 server 加 5 agent 不改变 server 成员、不自动迁移卷，验收清单包含调度隔离、版本/镜像/CA、DNS/CNI、存储/PVC/RWO/local-path 及工作台业务。已验收扩容写入来源基线后，再重新规划应用升级。

应用恢复只规划应用数据/配置恢复，显式 `restoreControlPlane=false`、`removeNodes=[]`，保留当前节点；扩容前的 generation、foundationSha256 或节点集合不匹配则拒绝备份。跨集群备份、数据资源映射不匹配、来源制品/schema不匹配、共享服务恢复超出所选消费者范围或包含 control-plane 数据均阻断。同 generation 的备份来源须与安装基线一致；更早 generation 的备份还需要历史谱系核验，本批尚未实现而明确阻断。新集群映射和整集群灾备不在本批范围。

## 本批验收及后续

本批验收包括原生 CLI 的文件读写/权限/不覆盖及执行阻断，Python publisher 与 Rust 的共同合法/非法契约语料，多来源/拓扑/组件选择、漂移与身份、迁移前置、数据去重与共享恢复、扩容阶段和节点保留。

后续依次实现可信首装记录持久化/只读采集与旧记录迁移、完整容量与RPO/RTO估算、备份及隔离恢复适配器、集群锁与冻结、升级执行与崩溃对账；每个执行能力分别做真实部署及断网验收后才可开放。本批的 fixture 验收不能替代生产或原生扩容验收。


## 2026-10-21 后续入口

新增 collect / migrate-record / inspect-source，详见 [操作手册](lifecycle-operations.md)。
plan 的 block 项新增 remediation，执行级指引位于 executionRemediations；均参与 planId。
纯 plan 仍不调用集群工具；collect 是显式指定工具和 context 的固定 GET 入口。迁移只输出待审候选，
组件 unknown、基线证据为空、健康未确认；不得将其自动接管为成功安装。发布端冻结报告核验器
已交付，现场内容可信接入与所有执行仍未交付，执行阻断码保持有效。
