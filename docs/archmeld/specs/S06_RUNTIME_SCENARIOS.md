# S06 · 场景、运行证据与时间线

## 1. 场景是运行分析单位

Scenario 包含目标、输入 artifact、命令 argv、cwd、环境 allowlist、工具链/构建、seed、预期性质、资源限制、采集方式、外部副作用和授权。源码 snapshot 与实际 binary/build digest 一起关联。导入来源不明的 Trace 可以浏览，但标为 unbound，不能验证某个 ChangeSet。

运行记录状态：queued、running、succeeded、failed、cancelled、timed_out；`succeeded` 仅表示采集任务成功，不表示业务契约通过。采集器开销记录为 measured/unknown，未测时禁止给出精确性能优化结论。

## 2. 首版格式与工具

必须实现 Chrome Trace Event JSON/VizTracer、OTLP trace JSON 的规范化导入，保存源 artifact；必须实现 Tracy 原生记录的保留、官方工具摘要导入与跳转。Tracy 转换可能损失部分事件：lossy=true 与 loss_notes 必填，不能声称无损统一。Perfetto 原生 trace 由本地 trace processor 查询，不把全部事件复制到主图。[SRC-VIZ] [SRC-TRACY] [SRC-PERFETTO]

时间线采用本地部署的 Perfetto UI/processor。默认不把 Trace 发往公网 UI；插件、资源和版本由 toolchain lock 管理。跨窗口消息校验 origin/source，不能照搬 demo 的无限制星号通信作为生产策略。[SRC-PERFETTO-EMBED]

## 3. 标准化对象

RuntimeRun、Process、Thread/Task、Span/Slice、InstantEvent、Counter、FlowLink、StateTransition、Artifact。纳秒时间戳与持续时间在 JSON 中为十进制字符串，避免 JavaScript 超过安全整数；保留 original clock_id、单位、零点、是否墙钟和校准误差。

具有 parent_span_id、flow/message/task ID 的才形成 observed causal edge。时间接近只作为搜索线索。跨进程/机器时钟未经校准不用于严格排序；OTel Link 不一定是树形 parent-child，应作为独立关联保留。[SRC-OTEL]

## 4. 源码关联

优先使用 build ID、源文件校验、symbol/debug info 与明确 probe ID。仅函数名匹配时标 candidate，不自动跳唯一当前符号。inline、优化裁剪、异步栈、生成代码要保留映射类型。无源码关联仍可保留事件，但 coverage 中说明 unmapped 数量。

点击事件默认打开当时 snapshot 的只读源码；切到当前代码需显式映射提示。每条相关架构边可展示哪些 scenario 支持它、哪些未覆盖，而不是“总调用次数”为零就判死代码。

## 5. 状态与数据流

候选状态机分四层：源码允许、运行观察、契约要求、待验证。探针明确记录 state owner、resource、旧/新状态、触发与 correlation。锁/队列只有必要标签默认不采集 payload；不记录密码、原始用户数据或全变量快照。

R1 必须支持场景比较：前后 CPU/等待/排队/资源区间和相关不变量。并发 Trace 不逐事件完全相等；按消息守恒、允许次序、资源释放、时限和状态契约验证。

## 6. 边界与容错

不完整 B/E 配对、负 duration、重叠不合法、孤立 flow、截断、压缩炸弹、嵌入 HTML 和路径逃逸均须检测。可保留可用片段，但返回 partial 并保存诊断。默认解压后大小/事件数量预算；超过预算由流式处理或明确拒绝，不让 UI 无限内存。

建议起始预算是本地可调整产品参数，不是性能实测：单导入默认上限 1 GiB、批量事件分页、UI 可见窗口查询；真实基准在 B026 测量并锁定。R1 至少测试一份百万事件级合成压力数据与真实场景，报告其硬件条件。
