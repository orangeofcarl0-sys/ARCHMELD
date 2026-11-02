# 2026-11-02 交叉审查闭合记录

审查对象：公开快照 `94468c6304be5c13892f6c84fc1a4145f994994f` 的 F01–F10。
本记录只覆盖当前项目测试阶段的可执行代码缺口，不宣称发布级或 R1 完成。
代码提交：`8286876005f81df8116c308a67f484df01e8b72e`，随后
`29997bd07aa53e4835628c1c153275080780ffe3`（F10 status 入口）和
`e0ed61d097986c5aa7f00ba7204df87d7201c9b1`（F09 module 投影）和
`63c7546e35f5477d0e8777e87831ec45c4b5346a`（F09/F10 完整入口、组件投影、
refresh、review-diff、HTTP 路由）和
`d2e538c124792e441f8012bb7189daa570c10844`（MCP status/review）和
`771d1b58d792ed0149682ef490ba3bf2ca5eefe6`（runtime span-parent edges）和
`3d6b7be449361e286bdb15c6f632af1db2cbc4b8`（real C++ SCIP import）；基线
`faf0c426783c1b4b4cbdea0d456a33599da3cd12`。

## 结论

F01–F10 已按审查中的可执行缺口完成到项目测试阶段：F01–F08 修正了代码级
缺陷；F09 增加模块/候选架构组件投影、手动位置、运行过滤和跨视图下钻；
F10 增加 status、review-diff、refresh 的 CLI/HTTP 入口及扩展项目发现。
仍保留明确残余：运行过滤不重建调用顺序，责任视图依赖显式选择的候选架构，
review 只做静态比较且不执行测试，未做“有/无 ARCHMELD”的同一 Agent 对比。
完整回归为 **199 passed / 0 failed / 0 ignored（72 个结果段）**，
VS Code 扩展 **37 passed / 0 failed**。

## 逐项状态

| 项 | 状态 | 实现与证据 |
|---|---|---|
| F01 MCP 工具表不一致 | 已闭合 | `tools/list` 只从 5 个真实 handler 生成；inspect schema 只列已实现 kind；其余契约工具由 `archmeld_capabilities` 报告 `requires_grant`/`not_implemented`，不冒充可调用工具。测试：`mcp_stdio.rs`、`security_mcp.rs`。 |
| F02 ContextPack 读文件头 | 已闭合 | 新增 `core::anchor::byte_range`，ContextPack、`read_source` 共用行/字节锚点解析；返回解析后的 byte range。测试覆盖第 262 行函数、CRLF、中文、无末尾换行：`context_pack.rs`。 |
| F03 先解析活动工作区 | 已闭合 | `archmeld index` 先捕获 snapshot，再把 CAS 内容物化到 ledger 自有目录，helper 只解析该目录；成功后删除副本。测试用一个在解析期间改写 live 文件的注入 helper，证明发布的图与旧字节一致：`cli.rs` 的 `index_tests`。另用打包的 `archmeld-index.exe` 真实运行：2 文件 → 4 entities / 3 relations，`analysis.removed=true`。 |
| F04 text 不搜源码 | 已闭合 | 新增 `kind=source` 的 snapshot-bound 字面量搜索，返回 path、行列、byte range、摘要、blob digest、entity_ids；支持 cursor 续扫。测试：`source_search.rs`。 |
| F05 ContextPack 非任务检索 | 已闭合 | objective 词项驱动名称/源码种子，加一跳关系邻域；claims 按相关性筛选，无关 rejected claims 聚合成 omission；cursor 绑定 snapshot+request，页面并集等于大预算结果。测试：`context_relevance.rs`。 |
| F06 20 万边截断不可达 | 已闭合 | 边扫描改为 `cap+1` 探测，命中即截断并把 coverage 标为 partial、写入 limitation 和 warning；不再声称完整无影响。测试包含真实 200001 边生产路径（36.36s，`pagination.edge_scan_truncated=true`、coverage partial），并保留缩小 cap 的快速分支测试。 |
| F07 分页丢跨页关系 | 已闭合 | 每页返回 `boundary_edges` 与端点 stub；合并页面按 relation id 可恢复 A→B→C 的 B→C。测试：`query_pagination.rs`。 |
| F08 长命令持有全局 Store 锁 | 已闭合 | scenario 拆为 prepare（Store）/execute（无 Store）/finalize（Store），HTTP 用 `spawn_blocking`；新增独立取消注册表和 `POST /v1/scenarios/{run_id}/cancel`。测试在 10 秒 scenario 运行期间，查询和取消都在 2 秒内返回：`scenario_http.rs`。 |
| F09 主图投影与布局 | 已闭合（测试阶段） | Modules 按首段路径聚合；Responsibilities 从显式候选架构的 `member_entity_ids` 聚合。两者按依赖方向分层、保留手动拖动位置、聚合边保留 `relation_ids`/`kind`/`evidence_ids`，模块/组件可下钻到成员源码，聚合边可查看底层关系和证据。`Filter Graph by Runtime Run` 只保留 run 映射实体，并把 `parent_span_id` 映射为 `span parent` containment 边；这些边不是 confirmed call，其他边仍是静态邻接。残余：span-parent 是包含关系而非调用语义，责任视图需显式选择架构。 |
| F10 工作流与入口 | 已闭合（测试阶段） | `archmeld status` / `GET /v1/status` 报告项目、最新快照、计数、语言、脏状态和允许动作；`archmeld review-diff` / `POST /v1/reviews/workspace-diff` 给出当前工作区对快照的行级变化、受影响实体、静态关系影响、触发条件、置信度和 `verification=not_run`；`archmeld refresh` 创建新快照并对旧快照做精确 diff，旧快照保留；VS Code 可发现项目和快照；MCP 暴露只读 `archmeld_status`/`archmeld_review`，只使用项目已注册的 active root。残余：review 不执行测试，未做同 Agent 对比实验，静态行证据不等于语义正确性。 |

## 回归命令

```text
TEMP/TMP = <home>/AppData/Local/Temp
PATH     = <home>/AppData/Local/Programs/Python/Python311;...
toolchain = 1.94.1-x86_64-pc-windows-gnu
cwd      = <repo>
argv     = cargo test --offline --no-fail-fast -p archmeld-core -p archmeld-api
exit     = 0
result   = 201 passed / 0 failed / 0 ignored / 73 sections
log      = sha256:9894aa1a52cccd63a505aa2280b36198159667cd12d205277680c7c3f1cdde9e
```

扩展：`node --test test/*.test.js` → 37 passed / 0 failed
（日志 sha256:52765088e7536981936bf8328fcfe3ef324de33fbfaf7efa4deedfbf0c13409f）。

环境说明：使用 Python 3.14 会让旧的 asyncio 夹具循环导入；系统
`C:\Windows\TEMP` 不可写会让 JFR 和 gopls 失败。上述最终回归使用 Python
3.11.9 和可写 TEMP/TMP，失败项均单独复跑通过；这不是代码修复。


## 真实 F10 流程

使用打包的 `archmeld-index.exe` 在临时仓库执行 `index → review-diff → refresh`：

```text
index:    sha256:8f75b81dea769c3da4bb3d0699ab4983a4c0c4b31e6409ff30b458ccc41e3a60, 2 entities / 1 relation
review:   state=changed, src/a.py entity=changed, verification=not_run
refresh:  state=changed, base 8f75b81… → new b309e21…, 1 file changed, 2 entities changed
```

旧快照保留，review/refresh 均未执行仓库命令。索引/审查/刷新 JSON 的
sha256 和输出结构记录在
`docs/release-evidence/CROSSREVIEW-2026-11-02.json` 的 `real_flow`。

## 明确限制

- Source search 是字面量、大小写敏感、非语义搜索；扫描预算 256 MiB，
  非 UTF-8 文件跳过并使 coverage 为 partial。
- ContextPack 的相关性是名称/源码词项加一跳关系启发式，不是语义模型；
  小预算下 header 可能放不下额外 `relevance`/`page` 元数据，此时会显式
  记录最终裁剪。
- F06 已执行真实 200001 边生产路径测试（约 36s）；该测试验证截断/partial，
  不是吞吐基准。
- Scenario 取消是进程内协作式；server 重启后活动标志不会持久化。
- Index 在 helper 失败时会保留已捕获但未索引的 snapshot，并在错误中说明；
  它不会把旧图绑到新字节。
- F09 的运行过滤把 span parent/child 映射为 `span parent` containment 边；
  它不是 confirmed call，也不能替代调用语义；责任视图依赖显式选择的候选
  架构，且没有做可用性对比研究。
- F10 的 status/review-diff/refresh 暴露在 CLI、HTTP 和 MCP；review 不执行
  build/test；未执行“有/无 ARCHMELD”的同一 Agent 对比实验。
- `archmeld status --workspace` 和 `review-diff` 只读且不持久化 drift；
  刷新必须显式运行 `archmeld refresh`，不会自动换快照。
