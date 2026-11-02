# 源码复用地图与工作仓库形态

## 1. 从真实底座开始

克隆并固定 CodeGraph，保留它的 Git 历史与上游许可证；建立自己的开发分支。禁止在初始化包目录运行无差别覆盖脚本。首次集成先通过原包相关测试，再添加 ARCHMELD crate 与扩展改动。

下列上游路径已经在本轮远程源码核查中确认：`Cargo.toml`、`crates/codegraph/`、`crates/codegraph-parser-api/`、各语言 parser crates、`crates/codegraph-server/`、`crates/codegraph-harness/`、`vscode/package.json`、`crates/codegraph/src/lib.rs`、`crates/codegraph/src/export/json.rs`。具体内部函数与文件名仍以固定提交为准，不根据本文假设未读取的私有 API。

## 2. 目标工作仓库（计划布局，不是当前包已有代码）

```text
<repo>bench/
  Cargo.toml                      # 保留上游 workspace，只增加成员
  crates/
    codegraph*/                   # 原解析器、引擎、服务；最小必要修改
    archmeld-core/                # domain/store/snapshot/query/plan/verify/policy
    archmeld-cli/                 # CLI、本地 API/MCP 入口、任务进程管理
    archmeld-contract-tests/      # 跨边界契约测试；不依赖付费模型
  vscode/                         # 从已有扩展薄改，不分叉整套编辑器
  webview/                        # 图/证据/计划/报告共享组件；按需抽出
  adapters/
    codeboarding/                 # 外部进程调用和候选导入配置
    languages/                   # 能力声明、工具配置、golden fixtures
    runtime/                     # VizTracer/OTLP/Tracy 等规范化适配
  contracts/archmeld/             # 本包 contracts 的版本化副本
  docs/archmeld/                  # 本包文档基线及后续 ADR
  tests/acceptance/               # 本包 AT 编号的真实实现
  tooling/                       # build/doctor/package；不下载未锁版本
  third_party/NOTICE             # 生成的依赖说明，不混入源码秘密
```

## 3. 上游改动预算

第一批上游改动仅覆盖：

**分析输出桥。** 在索引完成的明确边界增加版本化结构导出；保留原工具接口。新出口将节点、关系、来源范围、未解析项和文件计数一并导出。导出过程中拿到一致的索引视图，不能读到半更新状态。

**存储与网络隔离。** 明确传入 state/cache 根目录；禁止写用户既有 `~/.codegraph`；graph-only 不加载模型、不自动下载。若上游无对应配置则增加小补丁，而不是伪造已存在的命令参数。

**扩展入口。** 改产品标识与命令命名空间、保留署名；复用服务器发现、源码跳转、Webview 生命周期。禁用原有自动拉取引擎及与本产品冲突的后台索引/聊天入口；统一由 ARCHMELD core 管理。

**测试修复。** 仅根据复现修补上游，不先整体重构其内部。

## 4. 两条禁止混淆的数据通路

`CodeGraph -> EvidenceBatch -> importer -> SQLite` 是事实导入通路。

`Agent -> query/MCP -> Evidence/ContextPack` 是消费通路。

不通过让 Agent 调用几十次格式化的 MCP 文本输出，去反向猜全量原图；也不解析 HTML/Markdown 当稳定机器协议。原始后端 UUID/整数只保存在 backend_mapping，不能成为公开 Entity ID。

## 5. 进程边界

核心对外提供本地 HTTP API 和事件流，CLI 与扩展共享该核心；MCP stdio 入口是同一核心的薄适配。CodeGraph 默认作为受管理分析子进程，不直接给外部 Agent 不受控写能力。CodeBoarding、LSP、索引器和 trace processor 按任务启动。

首版无需把 Rust 主图与 SQLite 在同一内存进程强行融合。通过稳定 EvidenceBatch 让后端崩溃和替换不破坏账本；额外序列化成本用增量批次和压缩处理，而非牺牲快照隔离。

## 6. 删除与更新策略

不删除上游测试；与产品无关的运行入口可以不打包。fork 的每次上游修改记录在 `UPSTREAM_PATCHES.md`，包含必要性、受影响路径、是否提交 upstream、回归 AT。更新上游先在普通升级分支验证，不要求用户业务项目使用额外 worktree。
