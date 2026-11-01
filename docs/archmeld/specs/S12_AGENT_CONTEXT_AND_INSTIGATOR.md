# S12 · Agent 接入、上下文与 Instigator

## 1. 不重造 Agent harness

ARCHMELD 提供持久事实与受控动作，主编码 Agent 继续使用用户已有工具。首版以 MCP stdio 对接为必需；Codex 是一个可测客户端，不是核心绑定。其官方文档支持在本地客户端连接 MCP，具体配置以锁定客户端版本验证。[SRC-CODEX]

第一版不实现聊天账户、计费、多模型调度或全量会话存档。需要模型生成架构候选时，可运行 CodeBoarding 或让外部 Agent 读取 ContextPack、返回结构化 Claim/Plan。

## 2. ContextPack

包含 objective、non_goals、permissions_summary、snapshot/config、selected_entities、源码片段、graph slices、runtime references、claims/conflicts、verification requirements、unresolved_questions、budget与truncation。每个片段有 blob hash 和范围，不能只剩无来源摘要。

检索顺序：精确实体/符号 → 已知关系和程序路径 → 全文 → 可选向量。token budget 先保留目标、约束、未解决冲突和关键证据；省略内容保留可再取 cursor。不能把所有引用截断却给出“完整影响分析”。

模型输入输出保存 provider/tool versions、input digest、输出 artifact、数据授权摘要，不保存密钥；私密源码是否出站由项目策略决定。

## 3. 只读 Worker 与单写入者

主 Agent 控制当前任务，必要时并行只读调查；返回 Findings，不直接改事实。写入者仅在批准的 staging 中修改，通过 ChangeSet 核心应用；其他 Agent 通过同一 broker 访问，不共享任意 shell 写原仓库。

用户现有 Agent 若仍有原仓库 shell 权限，工作台不能承诺完全防止绕过。此类模式必须显示 externally_writable，检测变化、使计划失效；需要强控制时必须使用受限执行目录/OS sandbox，不靠提示词安全。

## 4. Instigator 的有限职责

Instigator 可关闭，默认旁路异步。订阅任务事件，发现已识别疑点停留在猜测、应运行的检查未运行、把已授权行动推回用户等行为时，发送局部 NUDGE。其消息绑定 task_revision、expiry 与已有任务范围。

它只能输出 NUDGE 或 SILENT；无 task.create/change.accept/approval/write 权限。无全局完成 barrier，不强制 worker 返工，不扩展目标、不替代具体调查。过期消息丢弃，不唤醒已结束任务；同一问题有冷却/次数预算，防止反复催促。

## 5. 调查深度质量

任务终点是可复核结果，不是回复长度。若发现未知且工具可用，主 Agent 应执行定向调查；若权限或资源阻止，明确阻碍与剩余不确定性，不能编造原因。核心保留 unresolved issue，Manager 可在下次环境变化后继续。

在同一问题中优先最小实验排除替代解释，不默认让多个模型辩论。质量由证据覆盖、反例与验证判定；另一个 Agent 的肯定不作为事实升级条件。
