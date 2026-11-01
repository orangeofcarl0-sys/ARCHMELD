# S04 · 分析适配协议与 CodeGraph 桥接

## 1. 适配器统一形态

采用受管理子进程，stdin/stdout 为版本化 NDJSON 控制/数据帧，stderr 仅日志。帧包含 protocol_version、request_id、type、payload；单帧上限 8 MiB，大产物传 artifact reference，不传 base64 整仓。启动握手报告工具版本、能力、支持平台、是否可能执行代码和是否需要网络。

核心统一控制 cwd、允许的环境变量、输入路径、输出目录、超时、内存预算、取消与子进程树回收。适配器不得自行修改目标仓库、安装依赖或调用外部模型。确需额外权限时返回 structured requirement，由核心批准。

协议操作：describe、analyze、cancel、health、shutdown。analyze 输入为 materialized snapshot 与构建上下文；输出按 S03 分批。禁止把 LSP 日志、进度提示或模型文本混入 JSON 控制流。

## 2. 已有出口与需要新增的出口

CodeGraph 固定版本已有图 JSON 导出函数，但本轮未确认一个满足 ARCHMELD 完整证据需求的现成 CLI 导出命令。因此 **`--archmeld-export-v1` 与 `--archmeld-state-root` 是本项目计划新增的参数，不是上游现成功能**。

桥接优先实现两处薄修改：索引达到稳定边界后输出全量/增量结构批次；把 cache/state 根目录显式传入。新参数只在 fork 内使用，不覆盖上游 `--run-tool` 行为。上游 graph-only 可用于避免不必要模型初始化，但仍需测试所有下载/缓存路径。

不能通过循环调用格式化 MCP 工具去抓取全量结构；也不能只取 export_json 的 nodes/links 就遗漏源码范围、错误和 coverage。

## 3. 映射规则

backend node integer/UUID -> backend_mapping；file/symbol/type/scope -> Entity；调用/引用/导入 -> 受支持的 Relation。无法识别的 upstream edge type 进入 diagnostics，不能自动改成 depends_on。没有可靠列范围时声明 line_only，禁止执行需要精确 range 的变换。

原解析器产生的关系默认 syntax 或 heuristic candidate；只有具备明确语义证据的适配器能生成 resolved 关系。解析器名字含 semantic 不构成精度证明。

文件覆盖清单必须由根枚举与工具实际处理清单交叉核对。父目录被忽略、文件过大、未知语言、语法错误要分别统计。

## 4. 稀疏 ID 与导出一致性

本地检查原图提供的真实迭代接口。测试 ID 含空洞、删除最大编号节点、重启恢复、增量删除文件后，导出数量与真实图内容一致。若现有导出依赖稠密编号，则添加真实迭代实现并回归；不得无限扫描到“看起来足够大”。

导出与更新同一后端不能交错造成悬空关系；通过暂停索引/读快照/受保护 generation 达成。导出的 entity/edge 数量必须与 batch manifest 一致，关系端点可解释为外部实体时也要明确声明。

## 5. 缓存和故障

每次 adapter run 分配独立工作区 namespace；单项目增量 cache 可复用，但 cache key 受解析器版本和配置控制。RocksDB 无法打开只影响该后端任务，不影响 SQLite 历史与报告。删除后端缓存前确认是本产品拥有的缓存。

崩溃退出保存 stderr tail、退出码、命令摘要，返回 failed；内存/时间超限返回 resource_exhausted/timeout；用户取消返回 cancelled。任何情况都不发布 partial 为 complete。允许一次按已配置恢复策略重试，不无限后台重启。

## 6. 适配器契约测试

每个适配器执行同一 suite：握手、版本不兼容、空仓库、错误语法、Unicode 范围、路径逃逸、批次中断、幂等、取消、无网络、错误 stdout、超大帧、scope coverage。插件只有通过后才进入 capability registry 的 verified 状态。
