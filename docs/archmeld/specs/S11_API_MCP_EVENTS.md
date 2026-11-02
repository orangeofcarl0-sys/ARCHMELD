# S11 · 本地 API、MCP、CLI 与事件

## 1. 一个核心，多入口

核心本地 HTTP API 是 UI/CLI 的统一状态入口；MCP stdio bridge 将有限高层工具映射到同一操作。不能让扩展、CLI 和 MCP 各自维护另一份计划状态或权限模型。OpenAPI 为 3.1，JSON Schema 为 2020-12，精确结构见 contracts/。

HTTP 默认 127.0.0.1 随机端口，启动时通过父进程受控通道交付 endpoint 与临时 token；不使用固定公开密码。浏览器/扩展不能把 token 放 URL。Origin/Host 校验、防重放/幂等与资源预算见安全规格。

## 2. 通用请求规则

所有快照相关查询绑定 project_id 与 snapshot_id；不可变对象按ID读取时由服务端恢复并检查其project/snapshot作用域。所有写请求含 idempotency_key；修改现有可变对象时必须带 expected_revision；新建分析/变更以明确base snapshot、契约digest和唯一请求键约束，不能凭默认current运行。长任务返回 Job，不能挂住 HTTP 连接假装同步成功。相同 key 相同 payload 返回原结果，不同 payload 返回 conflict。

ErrorEnvelope 包含 code、message、request_id、retryable、details、evidence_refs；不携带完整秘密命令环境。错误种类至少有 invalid_request、not_found、permission_denied、stale_snapshot、scope_conflict、tool_unavailable、unsupported、resource_exhausted、verification_failed、recovery_required、internal_error。

## 3. 高层 MCP 表面

首版声明工具不超过 16 个：14 个核心工具覆盖工作区状态、查找实体、取上下文、查询关系、运行场景、导入 Trace、管理架构候选、准备变更、执行已授权动作、查询验证与导出报告；另加只读的 `archmeld_status` 与 `archmeld_review`。详细输入 schema 与映射见 contracts/mcp-tools.json。[SRC-MCP]

当前实现只把有真实 handler 的工具放进 `tools/list`：`archmeld_inspect`、`archmeld_query`、`archmeld_context`、`archmeld_source`、`archmeld_status`、`archmeld_review`、`archmeld_capabilities`。其余契约工具由 `archmeld_capabilities` 明确标记为 `requires_grant`（需要 HTTP/CLI 外部授权，stdio 不能自批）或 `not_implemented`，不以“可用工具”公布。`archmeld_inspect` 的 schema 也只列出已实现的 kind。`archmeld_status`/`archmeld_review` 只使用已注册项目的 active root，不接受调用者路径；review 为静态比较，不执行 build/test。

工具 annotations 仅为客户端提示，不是授权。只读查询不接受任意 SQL；source 读取用已授权 Entity/Artifact，不接受自由绝对路径。写工具调用需要服务端权限和具体批准 digest，不能通过字符串 `approved=true` 自行批准。

## 4. CLI 语义

计划提供 `archmeld doctor`、`open`、`inspect`、`snapshot`、`query`、`scenario run`、`trace import`、`plan`、`change prepare/apply/recover`、`verify`、`report`、`mcp`、`serve`。这些是待实现的 ARCHMELD 命令，不是声称当前包已有可执行文件。

stdout 在 `--json` 模式只包含协议数据，进度/日志到 stderr。退出码规范：0 完成且满足请求；2 参数错误；3 缺工具/不支持；4 需授权；5 stale/conflict；6 验证失败；7 运行失败；8 结果不充分；130 用户取消。部分结果不以 0 默默掩盖 required 缺失。

## 5. 事件语义

持久事件包含 event_id、project_id、job_id、sequence、type、timestamp、snapshot、payload schema。sequence 在 JSON 中为十进制字符串；SQLite outbox 与业务状态同事务写入。订阅支持 after_sequence 重放；过期游标返回 resync_required 及可重拉状态，不静默漏事件。

事件如 snapshot.created、analysis.progress/completed/failed、claim.invalidated、change.state_changed、verification.completed、instigator.nudge。进度事件可合并，状态和授权事件不可丢失。消费者按 event_id 幂等，不把重复事件当重复 apply。

## 6. 兼容与生成

contract_version 以 major.minor 表达；破坏性改变增加 major，保留迁移说明。Schema 是字段真源，Rust/TypeScript 类型由它生成或经过等价性测试，不能各写一份漂移。枚举新增若旧客户端不能安全处理必须显式 unsupported。

OpenAPI 的每项 operationId 要映射 task/AT；MCP 调用实际 operation，不返回自造示例。协议测试至少跑真实进程与真实请求，fixture 仅作输入输出断言，不代替集成实现。
