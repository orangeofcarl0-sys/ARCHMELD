# ARCHMELD · 本地 Agent 构建初始化包

**包版本：1.0 / 规格基线：2026-10-30 / 交付类型：设计、契约与构建任务，不是已实现产品。**

目标：从现有开源工程出发，建设一个本地优先、多语言、证据驱动的代码理解与重构工作台。支持长期维护项目的 **Manager** 模式，也支持不污染仓库的一次性 **Expedition** 梳理、诊断与重构模式。

## 立即开始

用户只需把整个文件夹交给本地 Agent，并让它阅读 [START_HERE.md](START_HERE.md) 与 [LOCAL_AGENT_PROMPT.md](LOCAL_AGENT_PROMPT.md)。Agent 必须读取 [AGENTS.md](AGENTS.md)，按任务依赖执行，不先重写方案，不从空项目自行重造索引器。

主要入口：

| 入口 | 内容 |
|---|---|
| [方案总览](docs/00_BLUEPRINT.md) | 产品、两模式、架构、首版完整闭环 |
| [基础项目决策](docs/01_FOUNDATIONS.md) | 确定的底座、版本、替换边界、核查限制 |
| [源码复用地图](docs/02_REUSE_AND_REPO_MAP.md) | 从哪些上游目录开始，哪些保持，哪些新增 |
| [规格目录](specs/INDEX.md) | 模型、接口、语义、Runtime、UI、验证、安全等详细 spec |
| [首轮执行手册](execution/FIRST_SESSION.md) | 下载、锁定、上游基线、第一批提交 |
| [任务清单](execution/work-items.json) | 带依赖、产物、验收和禁止项的构建任务 |
| [验收矩阵](acceptance/ACCEPTANCE.md) | 需求 → 测试 → 任务的追溯入口 |
| [接口契约](contracts/README.md) | JSON Schema、OpenAPI、MCP、事件与状态机 |
| [来源与版本](research/SOURCES.md) | 官方依据、固定源码及未完成的实机验证 |

## 已冻结的关键决定

1. **从 CodeGraph 的固定源码提交做薄分叉。** 复用其 Rust 工作区、解析器、分析服务基础和 VS Code 扩展；不把原图直接当作可信事实库。
2. **新增 Rust 单体核心 + SQLite 证据账本 + 内容寻址对象存储。** 保持分析器可替换；CodeGraph RocksDB 仅是隔离、可重建的后端缓存。
3. **SCIP/LSP 提升语义，CodeBoarding 提供架构候选，Perfetto 与现成采集器提供运行证据。** 不自制编译器、IDE、时间线引擎或 Agent 平台。
4. **第一版必须交付双模式和理解—设计—变更—验证闭环。** 内部里程碑可以分步可用，但不准把半成品改名为 R1。
5. **默认单主 Agent、单写入者；Instigator 可关闭、异步、局部且非阻塞。** 无常驻多 Agent 管理树。
6. **安全修改通过暂存区、冻结契约、前置哈希检查和恢复日志实现。** 测试没跑、依赖没装、工具不支持，都不算通过。

## 当前状态与诚实边界

本包包含可供实施的设计文档、机器可读契约、合成示例、任务图与验收用例，以及仅用于检查本规格包的脚本。**不包含生产实现、不包含已通过的产品测试、不包含编译好的分析引擎。**

已在线核查 CodeGraph 与 CodeBoarding 的具体 Git 提交、部分源码接口和官方文档。当前运行环境不能直接克隆构建这些上游，因此未宣称完成依赖组合实测。其余语言工具与平台二进制必须由任务 B001 解析并锁定；包内明确标记 `needs_local_resolution`，不以虚构版本或空校验和冒充锁文件。

此前对话中的 ARCHMELD v0.1 原型与测试结论**不作为本方案基线**。本包不依赖该原型；旧文件不得未经审查混入新工程。

## 包检查

```text
python -m pip install -r tooling/requirements-validation.txt
python tooling/check_pack.py
```

检查只证明规格文件、引用、示例和追溯关系自洽，不证明产品已实现。详见 [验证说明](validation/README.md)。

## 范围与许可证

本文档与原创契约按 [LICENSE](LICENSE) 许可提供。后续产品以 Apache-2.0 为默认许可方向；上游署名、NOTICE、第三方组件与模型许可必须分别保留和审查。本包不重分发第三方源码或二进制。
