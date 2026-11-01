# 构建路线和依赖

按 [work-items.json](work-items.json) 的有向无环依赖执行。任务中的 spec ID 对应 specs/INDEX.md。每个任务的主要实现可以早完成，语言任务的完整验收须等 Runtime/变更/验证核心可用后执行，不能在依赖不存在时假造测试。

## 可用切片，不重命名半成品为R1

**V0：** B000–B008，真实结构索引、证据与源码图联动、双模式。这是内部可用切片，明确不具备完整语义/重构。

**V1：** B009–B018，语义提升、真实运行、架构候选与目标编辑。七语言适配按任务可并行准备，只有一个主写入者。

**V2：** B019–B023，冻结契约、受控变更、验证与MCP，完成第一条真实闭环。

**R1：** B025–B031完成全部强制测试、两平台交付、七语言和双模式；B024为可关闭增强，未启用不得成为主路径门禁；若启用也必须通过其验收。B032从首个UI开始持续维护，不等最后才写说明。

| 任务 | 依赖 | 产物目标 |
|---|---|---|
| B000 上游可重复基线 | 无 | 上游固定checkout；baseline/upstream-build.json；原有许可和NOTICE |
| B001 工具链获取、许可证与锁定 | B000 | toolchains.lock.json；third-party-notices；platform-matrix.json |
| B002 上游边界最小探测 | B000 | upstream-probes/；ADR适配修订；真实最小fixture |
| B003 证据核心和持久事务 | B000 | crates/archmeld-core/src/store；migrations；evidence integration tests |
| B004 快照、身份与源码位置 | B003 | snapshot/identity/source modules；hash test vectors |
| B005 CodeGraph桥接和首条真实索引 | B002, B004 | codegraph bridge adapter；first real repo import artifact |
| B006 统一查询与本地API | B005 | query/http/job modules；API contract tests |
| B007 代码—图的首个可操作工作台 | B006 | vscode/ARCHMELD UI；UI integration tests |
| B008 Manager与Expedition生命周期 | B006 | project/session manager；dual-mode lifecycle tests |
| B009 SCIP语义导入 | B004, B001 | scip adapter；semantic golden tests |
| B010 LSP与语言重构代理 | B009, B006 | language broker；rename preview tests |
| B011 Python与JS/TS语言包 | B010 | profiles/python、typescript、javascript；language evidence |
| B012 C/C++语言包及构建变体 | B010 | profiles/c-cpp；macro/virtual/function-pointer fixtures |
| B013 Java与C#语言包 | B010 | profiles/java、csharp；platform tool evidence |
| B014 Go与Rust语言包 | B010 | profiles/go、rust；language evidence |
| B015 运行证据规范化与时间线 | B004, B006 | runtime/importers；timeline integration；input limits tests |
| B016 场景执行与实际因果关联 | B015, B001 | scenario runner；capture manifest；causal tests |
| B017 解释调查与架构候选 | B006, B009 | claims/investigation；codeboarding adapter；candidate review UI |
| B018 As-Is/To-Be与可执行架构规则 | B017, B007 | architecture model/editor；rule registry and checks |
| B019 冻结契约与迁移计划 | B018, B004 | contracts/plan service；semantic consistency tests |
| B020 受控暂存与可审查变换 | B019, B010 | staging/transform/approval modules；diff UI |
| B021 安全应用、并发与恢复 | B020 | apply/recovery modules；fault injection suite |
| B022 独立验证与验收状态 | B019, B016 | verification engine；predicate runners；validation UI |
| B023 MCP与上下文包 | B006, B017, B019 | mcp server；context pack generator；external client smoke |
| B024 可关闭异步Instigator | B023 | instigator policy adapter；nonblocking tests |
| B025 持续漂移、重启与保留策略 | B008, B017, B021 | watcher/invalidation/backup/GC；restart tests |
| B026 规模、可取消与预算 | B007, B015 | benchmark suite/results；resource guards |
| B027 离线报告与模式提升 | B008, B018, B022 | report exporter；offline viewer；promotion tests |
| B028 统一安全与红队验证 | B021, B023, B016 | security acceptance suite；threat model review |
| B029 科学计算能力配置 | B022, B018 | scientific profile；golden data contracts；numerical checks |
| B030 七语言及混合项目完整验收 | B011, B012, B013, B014, B021, B022, B025, B026, B027, B028, B029 | release-evidence/；coverage matrix；known limitations |
| B031 双平台发行、更新与审计 | B030, B001 | VSIX/CLI bundles；SBOM；release notes；upgrade evidence |
| B032 可持续本地Agent交接 | B007 | docs/archmeld-progress.md；handoff checklist；operator guide |
