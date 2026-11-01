# 详细规格目录

这些文件是实现约束，不是已实现功能。对象字段以 contracts/schemas 为准；跨对象规则以 S19 为准；新增或冲突须提交 ADR，不允许悄悄缩减验收。

- [S01 · 产品范围、工作模式与用户旅程](S01_PRODUCT_AND_MODES.md)
- [S02 · 快照、源码位置、身份与失效](S02_SNAPSHOT_AND_IDENTITY.md)
- [S03 · 证据模型与事实存储](S03_EVIDENCE_AND_STORAGE.md)
- [S04 · 分析适配协议与 CodeGraph 桥接](S04_ADAPTERS_AND_UPSTREAM_BRIDGE.md)
- [S05 · 多语言能力与实现路径](S05_LANGUAGE_CAPABILITIES.md)
- [S06 · 场景、运行证据与时间线](S06_RUNTIME_SCENARIOS.md)
- [S07 · 理解引擎、架构模型与目标设计](S07_UNDERSTANDING_AND_ARCHITECTURE.md)
- [S08 · 工作台交互与视图验收](S08_WORKBENCH_UX.md)
- [S09 · 计划、变更事务、授权与恢复](S09_CHANGE_TRANSACTION.md)
- [S10 · 独立验证、行为契约与验收](S10_VERIFICATION_AND_CONTRACTS.md)
- [S11 · 本地 API、MCP、CLI 与事件](S11_API_MCP_EVENTS.md)
- [S12 · Agent 接入、上下文与 Instigator](S12_AGENT_CONTEXT_AND_INSTIGATOR.md)
- [S13 · 信任、执行隔离、隐私与供应链](S13_SECURITY_PRIVACY_EXECUTION.md)
- [S14 · 持久化、备份、恢复与日常运维](S14_PERSISTENCE_RECOVERY_OPERATIONS.md)
- [S15 · 构建、包装、安装与版本](S15_BUILD_PACKAGING_RELEASE.md)
- [S16 · 质量指标、资源预算与基准](S16_QUALITY_BENCHMARKS_AND_BUDGETS.md)
- [S17 · 科学计算与数值算法配置](S17_SCIENTIFIC_AND_NUMERICAL_PROFILE.md)
- [S18 · 三个端到端验收旅程](S18_END_TO_END_REFERENCE_CASES.md)
- [S19 · Wire 协议、哈希与语义一致性](S19_WIRE_HASH_AND_SEMANTICS.md)

按任务阅读，勿将全部内容每次重复传给模型。全局不变量读 S02/S03/S09/S10/S13/S19；第一纵向切片读 S04/S08；每种语言验收读 S05。
