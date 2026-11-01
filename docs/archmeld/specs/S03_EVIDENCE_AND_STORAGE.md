# S03 · 证据模型与事实存储

## 1. 模型边界

Entity、Relation、Evidence、Claim 分别保存。Entity 描述对象；Relation 表达类型明确的断言；Evidence 记录产生断言的依据；Claim 是解释/假设/设计决定。不存在无来源的“确定关系”。

来源类别为 syntax、semantic、runtime、human、model。它们不是由低到高的简单等级：human 可定义目标，但不证明实现；runtime 可证明一次发生，但不证明不存在其他路径。`confidence` 仅允许用于特定经过校准的估计，不代替来源和覆盖描述。

## 2. 关系语义

`references` 表示符号引用，不自动意味着调用；`static_call_candidate` 是候选调用；`resolved_call` 在声明构建条件下由语义工具解析；`observed_call` 需要运行父子/关联依据。`owns`、`publishes`、`consumes`、`guarded_by` 若不是直接工具输出，作为 Claim 或明确推断关系出现。

分派目标集合不止一个时保存集合及解析限制。运行中看见目标 A 不把其他候选删掉。SCIP definition/reference 与 Tree-sitter callsite 结合可建立解析调用，但必须在具体语言规则下验证作用域、位置和调用表达式，而不是引用出现在哪个函数内就算调用。

## 3. EvidenceBatch 原子导入

批次包含 run_id、adapter_id/version、project/snapshot/config、序号、覆盖、entities、relations、evidence、diagnostics。结束批次 final=true，并附最终 coverage 计数；core在完整批次校验后生成导入manifest。开始到结束全部在 staging 表/文件，校验通过后同事务发布 generation；中途崩溃不暴露半图。

每个关系引用至少一个存在的 evidence；语义 evidence 需工具版本和构建配置；runtime evidence 需 scenario/run/span 或事件关联；model evidence 不允许直接支撑 resolved_call。路径和 ID 逐项验证。无法解析的候选保留在 diagnostics/unresolved，不用空数组表示“没有调用”。

重复导入以 adapter/run/batch digest 幂等；相同 key 不同 payload 报冲突。并行分析采用独立 run_id，不能覆盖彼此。部分成功明确 coverage=partial，给出分母口径、排除数、失败数和未解析数量。

## 4. SQLite 逻辑表

最少包含 projects、snapshots、files、entities、entity_mappings、relations、evidence、claims、claim_support、architecture_models、architecture_memberships、scenarios、runtime_runs、changesets、approvals、verification_runs、jobs、events、artifacts、tool_runs。

源码与大型 trace 放内容寻址对象库，表中存 hash/size/media_type。SCIP 原文件与后端导出可保留为原始 artifact，不把所有原始字节复制进每个 entity。FTS 用于文本检索；向量索引非前置依赖且必须可重建。

SQLite 设置外键；使用单写入队列与 WAL。需要耐久性的 approval、apply journal、verification acceptance 必须真实 flush。具体 SQL migration 由 B003 实现，升级失败回滚；不得静默删除旧数据库重建来“修复”。

## 5. 查询输出

所有查询带 snapshot_id/config_digest、coverage、provenance、warnings、next_cursor。cursor 绑定查询和索引 generation，不能在索引刷新时跨版本拼接结果。没有结果可能是完整空集，也可能是未索引；必须可区分。

图查询有节点/边/深度预算，结果截断显式标记并可分页。对未授权源码返回 redacted，不返回假空文件。查询不能直接接受外部 SQL、任意磁盘路径或可执行表达式。

## 6. 冲突与历史

不同分析器的冲突先并存，保留各自配置和依据；只有明确规则才能合并。主界面可优先展示已解析关系，但不会删除矛盾证据。人类否定某项 Claim 记录 reviewer/reason，不修改原始运行日志。

证据保留支持重放结论；可过期但不可原地改写为新版本。对象库 GC 必须从批准计划、历史快照、运行/验证、导出引用的可达根遍历，引用对象不得删除。
