# 向导升级入口：方案与实现计划

状态：**草案（设计阶段，无代码改动）**。本文件只做方案与计划，实现须另行立项并事先声明验收范围。

## 1. 背景与问题

向导安装器（本仓库 runtime）当前的部署语义是三选一：

| 语义 | 现状 |
|---|---|
| 首装 | 支持（三种拓扑：single-k3d / single-k3s / multi-k3s） |
| 续装（同一次安装中断后继续） | 支持（checkpoint 恢复，身份三元组校验：配置哈希、bundle 身份、集群身份） |
| 升级已有部署 | **不支持**——preflight 检测到已有 K3s 数据即 blocked；检查点不匹配返回 409「不会自动接管或升级」 |

由此产生的现场困境：每发布一个新版本，已装集群没有升级入口。1015 现场批次已经出现过一次（旧 v7 集群不能自动接管，现场升级事实上悬空）。若要求现场走 chentu 原生 Helmfile 路径升级，等于要求现场操作员准备 ansible/helmfile/Python 环境并手工执行最高风险操作——违背向导的立项理由。

底层能力大部分已经存在，向导层缺的是入口与护栏：

- `helmfile sync` 的幂等协调（helm upgrade 语义）是 chentu 原生路径已验证的升级机制；
- nexus-first 全流程（import → promote → sync）本就是分阶段可恢复的；
- checkpoint 身份绑定（`resume.rs`）、chartPins/imagePins 钉定、`install-selection.json` 意图记录都可以复用。

## 2. 目标与非目标

### 目标

1. 向导在 preflight 发现已有宸途安装时，返回「可升级」而非一律 blocked，并展示旧安装的版本与身份。
2. 升级 = 新 bundle 的 nexus-first 流程重放：import 新 artifacts → promote → `helmfile sync`；数据（PVC/DB）与凭据不轮换、不重置。
3. 升级全程 checkpoint 化：升级前强制保存「旧 bundle 引用 + Helm release 快照」，任一阶段失败可续升（复用 resume 语义）；不提供自动降级。
4. 版本兼容性以发布清单**显式声明**（`upgradableFrom`），跨度外明确拒绝并给出指引；不做自动推断。

### 非目标（本计划不做）

- 任意跨度版本迁移或兼容矩阵自动推导。
- 数据迁移工具（PVC/DB 迁移仍属各组件 RUNBOOK 职责）。
- 无可信 checkpoint / 旧 v7 等不可识别集群的接管（维持 1015 边界）。
- 新的回滚工具（沿用 helm rollback 语义；「rollback 不回滚数据」的既有警示照旧）。

## 3. 方案

### 3.1 探测与兼容性声明

- **preflight 扩展**：现逻辑「检测到已有 K3s 数据 → blocked」改为分级：
  - 识别出宸途安装（实例 state 的安装清单 + 集群内 `chentu-deployment` 标注）→ 返回 `upgradeAvailable`，附旧版本、bundle 身份摘要；
  - 有 K3s 数据但不可识别 → 维持 blocked（原话术）。
- **发布清单声明**：`release-manifest.json` 新增 `upgradableFrom: ["0.1.0-dev.x", …]`（由 publisher 从上一冻结轮的 delivery-index 生成，不接受手填）；向导按此判断跨度，跨度外拒绝。

### 3.2 升级意图与 checkpoint

- 部署 API 增加升级意图（`api/deploy` 携带 `intent: "upgrade"` 或独立端点，实现时定）；向导 UI 在 `upgradeAvailable` 时提供两条明确路径：升级 / 保持不动。
- 复用 resume 的身份三元组校验绑定「旧安装」：配置哈希、旧 bundle 身份、集群身份任一变化即拒绝。
- 升级开始前强制 checkpoint：旧 bundle 路径、当前各 Helm release 的版本快照（chart 版本 + app version + values 摘要）。回退 = 用 checkpoint 里的旧 bundle 重放 sync。

### 3.3 执行路径（复用现有件，不建第二套排序）

1. **import**：新 bundle artifacts 导入集群注册表（复用 nexus-first import 与 `ALLOW_ONCE` 不可变语义；同 digest 跳过）。
2. **promote**：hosted 化，`imagePins`/`chartPins` 重绑到新版本。
3. **sync**：`helmfile sync` 增量协调。凭据/Secret 沿用既有值，不轮换（1015 原则）；升级前后凭据与数据比对进入验收门禁。
4. **阶段化推进**：沿用现有 deployment stages 划分（check → cluster → nexus → import → applications → access），向导 UI 无需新状态机。

### 3.4 失败语义

- 任一阶段失败 → checkpoint 保留，用户可续升（同 resume），也可停下；不自动回滚、不自动重试超过一次。
- 升级前置不满足（磁盘、架构、跨度外）→ 明确拒绝 + 指引文本，不做部分升级。

## 4. 实现计划（四阶段，可独立验收）

| 阶段 | 内容 | 主要文件 | 验收 |
|---|---|---|---|
| 1. 探测与声明 | preflight 分级探测；manifest `upgradableFrom`；instances API 暴露 `upgradeAvailable` | chentu: `bundle/hosted_release.py`（manifest 字段）、`bundle/install_catalog.py`；runtime: `instances.rs`、`bootstrap.rs` | 单测 + contract 测试：识别/不可识别/跨度外三态 |
| 2. 意图与 checkpoint | 升级意图 API；upgrade checkpoint（旧 bundle 引用 + release 快照）；跨度外 409 文案 | runtime: `wizard.rs`、`deploy.rs`、`resume.rs` | 身份不匹配/跨度外/正常绑定三分支单测 |
| 3. sync 重放 | nexus-first 升级模式（首装专有步骤幂等化）；stages 复用 | chentu: `deploy/helmfile/scripts/nexus_first.py`、`bootstrap/`；runtime: `bootstrap.rs` 阶段映射 | 本地 k3d「老包首装 → 新包升级」往返 + 凭据/数据比对脚本 |
| 4. 验收与文档 | 断网升级验收批次；README/init-help/docs；发布规则符合性声明 | `docs/`、验收 harness 新 scenario | 独立批次：Ubuntu 22.04 AMD64 断网，老包→新包，全阶段证据 |

依赖关系：阶段 1→2→3 串行；阶段 4 依赖 3。每阶段独立 PR，均不携带与本计划无关的改动。

## 5. 风险与边界

- **兼容跨度声明错误是最大风险**（升级直接作用于生产数据）。约束：`upgradableFrom` 由 publisher 从冻结上一轮 delivery-index 生成、不可手填；每个声明过的跨度都必须有对应验收证据后才可发布。
- **首装/升级路径漂移**会重演 k3d/k3s 统一时的整类缺陷。约束：阶段 3 必须复用同一 sync 代码路径，评审时按「不新增第二套排序/values 合并/Chart 安装循环」把关。
- **升级中断的中间态**（部分 release 已升级）依赖 helm 的逐 release 事务性；checkpoint 记录逐 release 快照，续升从第一个未完成 release 继续。
- 现场最小化假设不变：断网能力、Ubuntu 22.04 AMD64、无外部 DNS。

## 6. 发布规则符合性

- 新增目标（升级）须真实部署验收 + 断网独立验收，验收范围事先声明（本文件即声明草案，立项时按当时版本固化）。
- 不静默移除既有支持：preflight 的「不可识别集群 blocked」语义保留。
- 发布记录须包含：源码提交、包摘要、`upgradableFrom` 跨度、升级验收结果、已知限制。
