# Apeiron CLI 与 Chentu 的维护边界

`apeiron chentu …` 静态调用固定 Chentu crate。旧 `apeiron init/app/platform` 继续调用
同一个入口，保留参数、退出码与错误语义；不复制安装规则。SSO、Task、IAM、Zygos、
本机连接运行时、聊天和 scode 的宿主集成仍由 apeiron-cli 维护。

| 内容 | 归属 |
|---|---|
| 初始化网页、部署 API、执行与恢复规则 | Chentu |
| 原生 release、材料摘要、归属和依赖校验 | Chentu |
| 材料组装、profile 打包、冻结工具 | Chentu `bundle/release/` |
| Chentu 命令适配、版本 pin、build-info | apeiron-cli |
| 跨产品版本冻结、镜像构建协调、签名、上传与推荐通道 | apeiron-cli |
| Vasi 页面和同源 API 代理 | Vasi |

Python 兼容脚本只加载 `third_party/chentu/bundle/release/` 的固定实现。缺失子模块时直接
报错，不从 PATH 或其它工作区猜测实现。协调方显式传入审核过的分发 origin；内容地址、
摘要、归档布局、签名和验收要求不变。只运行 Python 材料工具的 CI job 也读取 gitlink
并检出同一个 Chentu 提交，不用默认 main。

`prepare-distro-material.py` 是旧 Minimal 材料兼容工具，不是新基础站点生成器。
新向导只接受完整的 v2 基础站点材料，并在交接后由 Vasi/Chentu API 管理应用。
本次不更新默认包、发行 recipe、已发布对象或推荐通道，不把旧包改标成新包。

## 测试归属

- HTTP 安装断言全部留在 Chentu。`test/chentu-contracts.test.js` 自动发现该固定版本的
  `rust/test/*.test.js`，针对内嵌 Apeiron 可执行文件执行；删除的是逐文件转发壳，不是断言。
- 浏览器套件清单在 Chentu `rust/scripts/test-browser-suite.js`，宿主仅提供二进制和
  `CHENTU_EMBEDDED=1`。八个浏览器流程保留；旧 Minimal/模型脚本由新的站点契约替代。
- 命令别名、选定发行材料、build-info 仍由 `test/chentu.test.js` 验证。注册隧道逐字节
  校验当前 Chentu 页面（只归一化运行时 CSP nonce），不再断言过时的模型控件。
- 材料归属与 CLI 摘要的原四项单元测试迁到 Chentu `test_release_material_ownership.py`；
  跨产品发行、签名与包组装集成断言留在本仓库，通过兼容入口测试真正的 Chentu 实现。

## 本轮验证

本机 ARM64：191 项 Rust 测试、Clippy、46 项 Python 发行集成测试、8 个独立无头浏览器
流程通过。Chentu 材料单元测试保留原四项断言，并增加 HTTPS 分发 origin 校验。
完整 Bun 测试 142 项通过（1692 个断言）。Chentu 的 `check-all.sh` 和完整 Helmfile 渲染也通过（0 错误、47 个既有规格警告）。

本轮迁移不执行真实站点变更；#373 已记录的 k3d 登录、更新及凭据/PVC/数据保留验收
继续有效，但不代表原生 AMD64、多节点、断网或正式发行验收。本地测试产物均在仓库外。
规格、测试归属及注册隧道期望页面的调整一并提交人审，不自动添加 N3 批准。
