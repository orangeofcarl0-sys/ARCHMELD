# 基础项目选择与锁定决策

## 1. 结论：薄分叉一个底座，组合专业组件

主底座确定为 `codegraph-ai/CodeGraph`，源码提交固定为：

```text
590c2ca7dd7b37bd1d41caf45fac535b6522f969
```

本轮读取的 Cargo.toml 与扩展 package.json 标记 0.21.0；源码含 Rust 图库、语言 parser crates、服务与 VS Code 扩展。源码 API 暴露图类型与 JSON 导出。[SRC-CG-MANIFEST] [SRC-CG-LIB] [SRC-CG-EXPORT]

选择理由是复用面足够直接：已有语言发现、语法提取、跨文件候选关系、图查询、编辑器入口和测试基础。它不是本方案完整产品，也不能替代语义工具、Runtime、事务化证据和安全重构。维护者体量或宣传性能不作为选型保证。

## 2. 复用与新增的界线

| 能力 | 决策 | 不允许的捷径 |
|---|---|---|
| 多语言语法与结构索引 | 复用 parser crates | 自写正则当解析器 |
| 候选调用/依赖与原图查询 | 复用、保留后端标签 | 直接宣称全部精确 |
| 项目事实存储 | 新建 SQLite 证据层 | 把后端 mutable ID 当稳定身份 |
| 符号导航 | SCIP 导入 + 既有 LSP | SCIP reference 直接改名为 calls |
| 架构解释候选 | CodeBoarding / 外部 Agent | 把 LLM 输出写成 observed 证据 |
| 图与代码联动 | 薄改现有扩展，复用 Cytoscape.js | 只提供截图或不可点击 Mermaid |
| Timeline | 本地 Perfetto + 专业采集器 | 静态序列图冒充 Trace |
| 变更执行 | 原生 refactor、LibCST/ast-grep、受控补丁 | 全仓文本替换、自动大改 |
| Agent | 标准 MCP 对接外部 Agent | 重造聊天/调度平台 |

## 3. CodeBoarding：架构候选层

固定研究提交 `a38a8d157150e78cee4e4c0f3f0646a4ae9ed938`。分析引擎/CLI 为 MIT，其他发行面不自动继承该结论。[SRC-CB]

使用独立虚拟环境与 JSON 文件边界导入，不把 Python 运行时嵌进核心。关闭遥测和未经授权的自动安装；不得假设一个关闭环境变量就已覆盖所有网络出口，应实测。支持配置用户模型，默认不设置付费密钥。无模型时仍可人工/外部 Agent 创建架构候选。

## 4. 不选择的主底座

不以此前临时原型为底座；不把 GitNexus 作为必需依赖；不分叉完整 IDE 或完整聊天 Agent 产品；不把 Sourcegraph 整套服务作为运行依赖。选择 SCIP 协议及具体索引器不等于部署 Sourcegraph。

## 5. 上游源码核查形成的首要探测

现有 `export/json.rs` 使用 `0..node_count` 与 `0..edge_count` 范围遍历后再取对象。该代码模式要求本地测试确认 ID 稀疏/删除/持久化恢复时是否完整；本包**不声称已经证明上游存在漏项 bug**。[SRC-CG-EXPORT]

B002 必须构造插入—删除—再次插入—持久化恢复—导出的序列，并比较真实迭代集合。若编号不稠密，添加真实存储迭代出口，而非按 count 猜最大 ID。其他必须探测：跨项目缓存污染、索引期间并发修改、启动自动下载、未解析调用误连、取消后残留进程。

## 6. 锁定策略

`research/upstream-selection.json` 是选择记录，不是所有依赖已安装的 lockfile。两个主源代码 SHA 已查证；其他工具记录来源、用途、许可证核查入口，状态为本地解析。B001 必须生成 `toolchain.lock.json`：平台、精确版本/commit、下载地址、SHA-256、许可证文件哈希、安装位置、执行权限与离线镜像状态。

禁止占位 SHA、`latest` 运行时下载或对未经读取文件编造 hash。工具升级独立任务执行同一能力回归；不能因上游新版本出现就默默更新。

## 7. 退出条件与最小替代

若 CodeGraph 某 parser 不满足金标测试，先修适配器或启用指定语义后端；不要重写整个项目。若上游引擎在目标平台无法构建，保留同一证据协议并用该语言的 SCIP/LSP 建立纵向路径，同时保留上游失败报告。只有核心 reuse 面经复现实验证明不适用才允许 ADR 更换主底座。
