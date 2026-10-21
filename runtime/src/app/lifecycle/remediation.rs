//! Operator actions describe evidence to obtain; they never authorize execution.
use super::*;
pub(super) fn guidance(code: &str) -> Value {
    let (owner, action, verification) = match code {
        "UPGRADE_PATH_NOT_UNIQUELY_VERIFIED" | "MIGRATION_RULE_NOT_UNIQUELY_VERIFIED" =>
            ("release-maintainer", "为实际来源清单、目标工件、拓扑、架构和完整组件组合声明唯一迁移路径，并通过冻结发布验收核验。", "使用通过核验的新发布清单重新生成计划；不得手改现场清单放行。"),
        "TARGET_SCHEMA_UNKNOWN" | "SOURCE_SCHEMA_UNKNOWN" | "DATA_CONTRACT_UNVERIFIED" | "LIFECYCLE_CONTRACT_MISSING" =>
            ("component-maintainer", "补齐对应来源及目标组件的配置、数据 schema 和数据角色契约；未知项须经实测确认。", "新发布契约与实际来源工件一致后重新规划。"),
        "BASELINE_UNVERIFIED" | "COMPONENT_STATE_UNKNOWN" =>
            ("site-operator", "先运行 lifecycle collect 和 migrate-record，核对旧记录、实际组件工件、配置及数据归属；待审记录不能作为已验收基线。", "逐项取得基线验收证据并重新采集；禁止由 resume 检查点推断安装成功。"),
        "CLUSTER_IDENTITY_MISMATCH" | "BACKUP_CLUSTER_MISMATCH" =>
            ("site-operator", "核对显式 kubeconfig/context 与集群 UID；为该集群重新采集并选择其专属记录、意图及备份。", "所有输入绑定同一集群和安装身份；不得修改 UID 掩盖错用。"),
        "STALE_BASELINE" | "BASELINE_DRIFT" | "RECORD_IDENTITY_MISMATCH" | "COMPONENT_DRIFT" | "UNEXPLAINED_DRIFT" | "CLUSTER_UNHEALTHY" =>
            ("site-operator", "诊断健康与漂移，核对当前已验收 generation、配置、节点和组件；解决后重新采集。", "使用同一新鲜基线的完整输入重新规划；不复用旧计划。"),
        "FOUNDATION_CHANGE_UNSUPPORTED" | "COMPONENT_SET_CHANGE_UNSUPPORTED" | "NEW_DEPENDENCY_REQUIRES_SEPARATE_CHANGE" | "MIXED_OPERATION_UNSUPPORTED" =>
            ("change-owner", "将基础设施、组件增删或新依赖拆成独立变更，单独实现并验收。", "独立变更验收并提交新基线后重新生成应用计划。"),
        "POST_EXPANSION_BASELINE_REQUIRED" | "BACKUP_PREDATES_ACCEPTED_BASELINE" =>
            ("site-operator", "先单独验收扩容并更新基线，再采集新节点状态并重新生成应用备份。", "应用恢复使用扩容后的基线和备份，保留已验收节点。"),
        "EXPANSION_EXECUTOR_UNIMPLEMENTED" | "EXPANSION_TOPOLOGY_UNSUPPORTED" | "EXECUTOR_UNIMPLEMENTED" =>
            ("platform-maintainer", "实现并独立验收该操作的执行器；本版本不提供执行放行。", "提交明确支持范围、真实及断网验收证据，再审阅具体执行计划。"),
        "EVIDENCE_CONTENT_VERIFICATION_UNIMPLEMENTED" =>
            ("platform-maintainer", "发布端须核验冻结报告及附件；现场仍须实现可信证据接入，不能仅相信清单中的摘要。", "发布核验成功不解除现场执行阻断。"),
        "ACCEPTANCE_REQUIRED" | "LIVE_REVALIDATION_REQUIRED" =>
            ("change-owner", "完成实际场景验收、实时状态复核及具体计划审阅。", "快照 ready 和发布契约验收均不等于生产授权。"),
        "EFFECTIVE_VALUES_VALIDATION_UNIMPLEMENTED" =>
            ("platform-maintainer", "通过唯一 Helmfile 入口补齐有效配置渲染与差异核验，保留现场配置。", "完成实现及配置保留验收后重新规划。"),
        "FULL_CAPACITY_ESTIMATE_UNIMPLEMENTED" | "CAPACITY_ESTIMATE_INCOMPLETE" | "BACKUP_CAPACITY_INSUFFICIENT" | "PROTECTION_BUDGET_MISSING" | "PROTECTION_POLICY_REQUIRED" =>
            ("backup-owner", "补齐保护策略与实际容量、暂存、独立副本、恢复演练和新旧卷共存预算，以及实测 RPO/RTO。", "保护及恢复所需预算全部满足后重新规划。"),
        "APPLICATION_RESTORE_CANNOT_RESTORE_DATASTORE" | "SHARED_RESTORE_SCOPE_EXCEEDS_SELECTION" =>
            ("recovery-owner", "缩小到已验收的应用数据恢复范围；共享服务或控制平面恢复须独立计划。", "不得覆盖未选消费者，亦不得自动撤销已验收扩容。"),
        "BACKUP_SET_REQUIRED" | "BACKUP_COVERAGE_INCOMPLETE" | "BACKUP_DATA_MAPPING_MISMATCH" | "BACKUP_RESTORE_ADAPTER_UNVERIFIED" | "BACKUP_RESTORE_NOT_VERIFIED" | "MIGRATION_RECOVERY_UNVERIFIED" | "DATA_BINDING_MISSING" | "DATA_OWNERSHIP_MISMATCH" =>
            ("backup-owner", "核对数据归属与共享消费者，补齐备份适配器、独立加密副本、取回解密与隔离恢复证据。", "生成覆盖本次操作的新鲜恢复集并重新规划。"),
        "BACKUP_SOURCE_BASELINE_MISMATCH" | "BACKUP_SOURCE_CONTRACT_MISMATCH" | "RESTORE_SOURCE_CONTRACT_MISMATCH" | "RESTORE_COMPONENT_STATE_MISMATCH" | "RESTORE_ARTIFACT_MISMATCH" | "SOURCE_CONTRACT_STATE_MISMATCH" | "MIGRATION_PREREQUISITE_MISMATCH" =>
            ("recovery-owner", "核对实际来源版本、组件工件、schema、配置与备份来源；补齐匹配的旧制品。", "来源契约与现场或恢复集完全匹配后重新规划。"),
        "HISTORICAL_BACKUP_LINEAGE_UNVERIFIED" | "BACKUP_REUSE_UNIMPLEMENTED" =>
            ("platform-maintainer", "实现并验收历史基线谱系或备份复用核验；当前须重新生成匹配基线的备份。", "不得修改 generation 或来源版本绕过谱系验证。"),
        "COMPONENT_NOT_INSTALLED" | "TARGET_COMPONENT_MISSING" | "TARGET_PLATFORM_UNSUPPORTED" | "EXPANSION_NODES_MISSING" | "EXPANSION_NODE_ALREADY_PRESENT" =>
            ("site-operator", "核对组件选择、目标支持矩阵及新增节点清单，修正变更意图或选择支持该现场的发布包。", "重新生成独立计划并审阅完整差异。"),
        _ => ("platform-maintainer", "该阻断项缺少专用处理流程；停止操作并补齐对应实现与验收证据。", "保持阻断，不通过编辑摘要或状态绕过校验。"),
    };
    json!({"owner":owner,"action":action,"verification":verification,"allowsExecution":false,
        "reference":"docs/lifecycle-operations.md"})
}
