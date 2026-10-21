# 20261016.1 AMD64 安装前置问题复现

固定来源：交付包 `apeiron-install-0.1.0-dev.20261016.1-linux-amd64.tar`，7359795200 字节；
交付记录 SHA-256 `7fb90d7a27dd1411cd089343c42b94b2005c55400393f9e5469f03f914761196`。
本轮从该实际 tar 中提取二进制、发布清单、setup/install.json 和 K3s artifacts.yaml；未改写旧文件。
交付记录的整包摘要是已有 provenance，本轮没有重新对整个 7 GB 包进行完整摘要验收。

发布清单原始字节 SHA-256：`6462e76c3baf4f474919020d455722a6f494444b3f6b737b65c69572dba948c4`。
固定 CLI 提交 `044199d6b468f300e8de95011e5c8ef1cfa3bebf`，Chentu 提交
`0bc39fea4571b085c80b7d985ce35926aa063588`。提取的原二进制可在本 AMD64 环境运行帮助命令。

通过 `lifecycle inspect-source` 运行相同的纯 catalog 代码路径；与上述旧 CLI 提交逐字比较，
`catalog()` 到 `airgap()` 的完整代码一致，SHA-256 均为
`fd50e84aa278f575e5ea2bd4cd2193fa4482f6c8d0c98f1e2385077dd4a790fc`。
因此本轮复现的是旧版实际资源在原样前置校验代码上的拒绝，不是把新规划器的拒绝当作旧安装失败。

| 目标 | 实际结果 | 结论 |
|---|---|---|
| single-k3s / amd64 | catalog 可解析；k3s-amd64.supportedTopologies 只有 multi-k3s | 原包未声明单机 K3s 支持，不可据此建立该拓扑的验收基线 |
| multi-k3s / amd64 | catalog 可解析；拓扑已声明 | 只证明通过这两项前置，未进行实际安装验收 |
| single-k3d / amd64 | 未声明；catalog 报“K3s 版本或架构与固定基础镜像包不匹配。” | airgap 兼容补丁仅支持 k3d-arm64，不能推断 AMD64 可用 |

安装 catalog 只提供 k3s-amd64，K3s 声明为 v1.36.3+k3s1/AMD64。
兼容补丁不会为 AMD64 安全生成基础镜像；移除架构校验会将 ARM64 工件错当 AMD64，不能作为修复。

原始提取物、source-inspection.json、source-comparison.json 和旧源码片段保存于仓库外
`/root/repos/lifecycle-validation/amd64-repro/`。新增 CLI 集成用例冻结同样的两类拒绝行为。

本轮没有部署旧版本或修改任何集群。未复现完整业务首装、断网闭包或升级成功；PR 评论中另一交付
工作区的 upgrade-attempt-1620/report.md 未在当前工作区找到，不将其作为本轮新增验收证据。

处理建议：现存 multi-k3s 现场应通过只读采集确认实际旧基线，不能一概认定旧版 AMD64 全部不可安装。
如需实验室复现 single-k3s/single-k3d 源线，须发布新的不可变补丁版本、补齐资源和拓扑实测；不覆盖旧包，
也不在本次 PR 中擅自扩大旧版支持矩阵。外网 single-k3s 应按其实际来源包单独核实，不能沿用内网结论。
