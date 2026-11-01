# 机器可读契约

这里定义未来 ARCHMELD 实现必须遵循的接口；不表示已有服务可调用。

| 文件 | 用法 |
|---|---|
| [JSON Schema](schemas/archmeld.schema.json) | Draft2020-12 公共定义；单对象 wrapper 文件便于验证 |
| [OpenAPI](openapi.json) | 本地 HTTP / 作业 / 读取 / 提案 / 变更 / 验证接口 |
| [MCP tools](mcp-tools.json) | 给外部 Agent 的高层工具，不含人工批准能力 |
| [adapter protocol](adapter-protocol.json) | 解析器 NDJSON 批次桥接、能力与取消 |
| [state machines](state-machines.json) | 合法状态迁移；由服务器执行守卫 |
| [language profiles](language-profiles.json) | 首版支持目标、证据和局限，不虚报已验证 |

`https://archmeld.invalid/` 只是本地 JSON Schema 命名空间，校验器必须使用本地 registry，禁止访问网络下载。普通 HTTP OpenAPI 使用相对本地文件引用。MCP inputSchema 内联传递依赖定义，客户端无需外部解析。

语法检查只能确认字段合法；引用、哈希、作用域、权限、审批、DAG、证据等级和验收完整性由 [S19](../specs/S19_WIRE_HASH_AND_SEMANTICS.md) 规定。examples 是合成数据，既不代表实际分析结果，也不代表产品测试已运行。

协议扩展只在明确能力协商后使用。不要把未知字段自由写入核心记录；需要扩展时增加版本/适配器原始 artifact。所有客户端都通过同一核心权限与状态机，不维护自己的“已完成”状态。

数据库逻辑表与事务映射见 [STORAGE_AND_TRANSACTION_MAP.md](STORAGE_AND_TRANSACTION_MAP.md)；每个API操作的任务和验收对应见 [operation-coverage.json](operation-coverage.json)。
