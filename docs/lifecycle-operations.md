# 生命周期：旧记录采集、迁移与阻断处理

本批新增显式只读采集、旧检查点迁移候选和补救指引。所有生产升级、扩容、备份及恢复仍禁止执行。
安装包构建不读取现场；下述命令只在现场操作机生成独立私有文件。内外网分别采集和审阅。

## 1. 采集旧现场

```sh
apeiron lifecycle collect --kubectl /absolute/bin/kubectl \
  --kubeconfig /external/cluster/kubeconfig --context reviewed-context \
  --manifest /external/source/setup/release-manifest.json \
  --legacy /external/site/config.json.installation-resume.json \
  --configuration /external/site/config.json --topology multi-k3s --network offline \
  --out /external/evidence/collection-new.json
```

必须显式指定 kubectl、kubeconfig 和 context；不会选取默认生产 context。只运行固定 GET：
kube-system Namespace、Nodes、全命名空间 Pods 和 PVC；首尾重读集群 UID 和节点快照，不一致即拒绝。
每个请求有 15 秒服务端超时、20 秒进程超时和 4 MiB 输出限制。不得使用有不可信 exec 插件的 kubeconfig。
RBAC 最小范围为这些资源的 get/list，不读取 Secret，不运行 exec、apply、sync、升级或恢复。

输出 0600、新建且不覆盖。输出保留节点身份/角色/架构/K3s 版本和 Ready、运行镜像 ID、PVC 元数据；
配置和旧检查点只记摘要，不复制凭据、Pod 环境或 annotations。输出仍含现场元数据，应留在仓库外。
采集是顺序读取，不是原子快照；首尾一致不证明中间无漂移，也不证明应用健康或安装完成。
未知 role（例如缺失 server 标记）、混合架构、过大响应、读取失败不能降级为成功。

## 2. 迁移旧 resume 检查点

```sh
apeiron lifecycle migrate-record --collection /external/evidence/collection-new.json \
  --manifest /external/source/setup/release-manifest.json \
  --legacy /external/site/config.json.installation-resume.json \
  --configuration /external/site/config.json --installation-id reviewed-installation \
  --out /external/evidence/migration-new.json
```

支持 `schemaVersion=1/checkpoint=bootstrap-complete/commandActive=false` 的旧代检查点，包括没有
installationId/generation/clusterUid 的兼容记录。固定旧 CLI 044199d 已写入 clusterUid/configHash，
不能把无 UID 样本概括为该版本所有检查点；若旧记录已有 UID/configHash，必须匹配采集。四个输入文件
按原始字节绑定，configHash 单独按旧 resume 的规范 JSON 算法比较。不沿用旧目录、PID、environment
或命令，不修改原检查点，也不放宽原首装 resume 校验。

结果为 `chentu-installation-migration/status=review-required`，包含独立 v1 `installationCandidate`：
显式安装 ID、采集到的 UID/节点/拓扑/架构、声明来源版本及配置摘要、起始 generation=1。
同时输出绑定该候选摘要的 observationCandidate，健康状态按未验证处理、含漂移阻断。network 是操作者
显式声明的模式，不代表已完成断网验收。两者可用于只读分析，不能直接放行执行。
**generation=1 是新候选序号，不是一次成功安装或已验收扩容的证明。** 所有组件保持 unknown，
不会把包内组件推断为已安装或未安装；逐组件配置和基础配置的 64 个零摘要仅为未知占位符。
baselineEvidence 和 acceptedExpansions 为空。该候选须以采集报告配对保存，不得单独称为可信基线。

仍需核实组件的实际 Chart/镜像工件、配置/data schema、数据归属、有效 values、基础设施配置和应用
健康，并取得真实基线验收证据；自动完成这些核实的适配器尚未交付。本入口完成保守导入与缺口诊断，
不承诺把任意旧集群自动接管为已验收安装。原现场的既有扩容须重建可追溯记录，不能丢失后冒充首装。
这与设计中“未知旧集群不自动接管”的边界一致。采集文件可被修改，不是带可信签名的证明；现场执行
仍要求实时重验、内容核验和具体计划审阅。

## 3. 阻断项的补救

计划保留原 `code/subject`，新增 `remediation`：owner、action、verification、reference 和
allowsExecution=false；执行级阻断在 executionRemediations 中逐项解释。补救说明参与 planId 摘要。

| 阻断 | 责任与解除条件 |
|---|---|
| BASELINE_UNVERIFIED / COMPONENT_STATE_UNKNOWN | 现场核实组件、数据归属和配置，取得验收证据；不能用 resume 状态冒充成功 |
| UPGRADE_PATH_NOT_UNIQUELY_VERIFIED / MIGRATION_RULE_NOT_UNIQUELY_VERIFIED | 发布维护者为精确版本工件、拓扑、架构及完整组件组合声明路径，并核验冻结报告 |
| SOURCE_SCHEMA_UNKNOWN / TARGET_SCHEMA_UNKNOWN | 组件维护者实测并补齐来源/目标 schema 契约 |
| FOUNDATION_CHANGE_UNSUPPORTED | 基础设施变更独立实现验收并提交基线；不能借应用升级改 K3s/存储/Nexus |
| POST_EXPANSION_BASELINE_REQUIRED | 内网 3 server 加 5 agent 必须先独立验收，再采集新基线及应用备份 |
| BACKUP_PREDATES_ACCEPTED_BASELINE | 重新生成扩容后的备份；应用恢复保留已验收节点 |
| EVIDENCE_CONTENT_VERIFICATION_UNIMPLEMENTED | 发布端核验器已交付；现场证据可信接入仍未开放，不能仅信 manifest 摘要 |

其他码均有按操作类别的说明；未知新码默认维持阻断，不提供绕过方法。实施上述补救不构成生产授权。

## 4. 旧源包安装能力诊断

```sh
apeiron lifecycle inspect-source --root /external/source/chentu \
  --manifest /external/source/setup/release-manifest.json --architecture amd64 \
  --out /external/evidence/source-inspection-new.json
```

只读取发布清单和安装 catalog，分别报告 single-k3s、multi-k3s、single-k3d 的声明范围及 catalog 解析。
这不是安装、离线闭包或运行验收，`installationVerified=false`。不改旧包支持矩阵或补写架构制品。
真实 20261016.1 的诊断与证据边界见 [旧版 AMD64 复现](legacy-amd64-reproduction.md)。

## 旧检查点来源的显式对勘（副本演练补充）

安装目录名、发行包目录及真实 resume 绑定清单可能不同。使用 `migrate-record` 时可显式提供
`--legacy-manifest CHECKPOINT_SOURCE`，与 `--manifest SOURCE` 按原始文件 SHA-256 比较。
两者不一致时拒绝生成迁移候选，不回显导入内容、不读取 checkpoint 中的路径或运行命令。
路径由操作员独立核实后传入；来源文件匹配仅标记 `sourceProvenance=operator-supplied-manifest-match`，
候选仍为 review-required、liveVerified=false、executionAllowed=false。
没有显式来源文件时标记 sourceProvenance=unverified，不能仅凭旧目录存在而验收来源版本。
