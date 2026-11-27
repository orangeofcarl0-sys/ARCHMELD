# ARCHMELD 自分析问题深挖，2026-11-26

本文是 [`ARCHMELD-SELF-ANALYSIS-2026-11-26.md`](ARCHMELD-SELF-ANALYSIS-2026-11-26.md)
的续篇：不再按“发现”叙述，而是把同一批证据拆到数据模型、查询面、快照策略和
工具链四层，回答三个问题：**哪些是真问题、根因在哪、先修什么**。

证据仍来自 `docs/evidence/self-analysis-2026-11-26/`；新增的模型探针输出是
`model-probe.txt`，脚本是 `scripts/model_probe.py`。所有数字对应快照
`sha256:66aaec6d…`（HEAD `7b1dc280`，1,828 文件 / 34,042,848 字节，
742 个文件进入解析，11,043 实体 / 46,244 关系）。

## 0. 口径

- **缺陷**：行为与契约/文档/直觉不一致，且会让用户得到错误结论。
- **覆盖缺口**：数据确实没进来，且没有在 coverage 中说明。
- **语义缺口**：数据进来了，但维度/属性被丢掉，导致问题无法回答。
- **可用性问题**：结果正确但不可用（太慢、太大、报错不可读）。
- **方法学问题**：分析过程本身的偏差，不是产品行为。

严重度：P0 会让核心结论错误或为空；P1 会让分析结论系统性偏斜；P2 是覆盖/
可复现性缺口；P3 是体验/性能。

## 1. 问题总表

| ID | 严重度 | 层 | 问题 | 对应前文 |
|---|---|---|---|---|
| P01 | P0 | 查询/上下文 | 显式种子在高连接实体上被裁掉，context 返回空包 | F1 |
| P02 | P0 | 快照/漂移 | drift 与 capture 两套省略策略，干净树报 6,623 added | F2 |
| P03 | P1 | 语义 | `needs_review` 在 status 与 review 中定义不同 | F2 子项 |
| P04 | P1 | 图模型 | 28,818 条 call candidate 被通用名主导，跨 crate 耦合不可读 | F3 |
| P05 | P1 | 图模型 | 导出边的 `resolved_by`/`is_direct` 被丢弃 | 新 |
| P06 | P1 | 图模型 | 5,264 条重复边被静默折叠，关系计数不是调用点计数 | 新 |
| P07 | P1 | 证据 | bridge coverage=`complete`，evidence coverage=`partial` | 新 |
| P08 | P1 | 图模型 | Kotlin/TOML/Bash 605 个实体被标成 `unknown` | F4 |
| P09 | P1 | 解析 | 3 个 Kotlin parser error 只有计数、没有路径、也没进 coverage | F6 |
| P10 | P1 | 语义 | `brief` 的 `confirmed` 实际是 `declared`，没有验证 | 新 |
| P11 | P1 | 查询 | 测试实体没有 role 维度，impact/fan-in 混入测试 | 新 |
| P12 | P2 | 图模型 | 151 个 module 名与 type/symbol 同名，且 module 无 anchor | 新 |
| P13 | P2 | 索引 | 1,828 个捕获文件里 1,086 个从未进入解析，原因不可见 | 新 |
| P14 | P2 | 索引 | 默认排除 `examples/`，产品自己的 bridge 示例不在图里 | 新 |
| P15 | P2 | 图模型 | 40 个 `parse_source` 实现只对应 4 条 implements 关系 | 新 |
| P16 | P2 | 索引 | vendored `cytoscape.min.js` 与 Cargo.toml 配置节点污染排名 | F5 |
| P17 | P3 | 产品面 | 仓库没有 `archmeld.map.txt`，map 面未自用 | F8 |
| P18 | P3 | 展示 | snapshot/index/brief 的 coverage 口径没有同时展示 | F9 |
| P19 | P1 | 身份 | snapshot 绑定工作区字节而非 git 内容，CRLF 变化即换 snapshot | 新 |
| P20 | P2 | 身份 | `index` 默认包含 untracked 文件，status 不显示 origin | 新 |
| P21 | P1 | 身份 | 账本不绑定 CLI/helper 构建版本；`--version` 只有 0.1.0 | F10 |
| P22 | P2 | 证据 | 关系证据是 artifact 级（整个 export），没有调用点位置 | 新 |
| P23 | P3 | 性能 | 每次 freshness 全量哈希 2.5 GB；auto brief 扫两遍 | F7 |
| P24 | P3 | 可用性 | MCP status 1.36 MB、review 5.4 MB，无分页/摘要 | 新 |
| P25 | P3 | 打包 | helper 缺 DLL 时退出码未解码、stderr 为空 | 新 |

## 2. P0：两个会让核心结论错误的问题

### P01 context pack 把显式种子裁掉

证据：`context-type-seed.json`。对 `crates/archmeld-core/src/store/mod.rs::Store`
（type）传入 `entity_ids`，objective 为“explain how the SQLite ledger…”，
预算 4,000 / 8,000 / 16,000 token：

```text
selected_entities=0  source_snippets=0  graph_slices=0
pack bytes=2,241/2,241/2,242
omissions=[{kind:omitted, count:473/591/848, reason:final_budget_trim}]
```

这不是“预算太小”：2,241 字节远低于 64 KB；也不是“种子不存在”：同一 ID 的
`inspect`/`source` 正常。根因链（`context.rs`）：

1. 实体元数据和第一段源码先被加入，随后关系循环对每条关系做 `append_fits`；
   放不下的关系写入 `pack["omissions"]`，但 **omissions 本身不参与预算**，
   每条带一个 70 字符 relation id，最终可膨胀到几十 KB。
2. 最终裁剪按 `graph_slices → evidence_refs → source_snippets → non_goals →
   selected_entities` 顺序 pop，直到 JSON 放得下；omissions 最后才压缩。
3. 没有任何“显式请求的实体是必需头”的规则；裁剪后也不会重建。

影响：实体越核心、关系越多，越容易拿到空包；MCP agent 的第一反应会是“没有
上下文”，而实际上证据都在。修复：把显式种子元数据 + 第一条 anchor snippet
设为 mandatory header（和 objective 同级）；omissions 先聚合/限流再计入预算；
优先裁 relevance/page/omissions，最后才裁内容；加一个默认预算下 `Store`
必须非空的回归测试。

### P02 drift 与 capture 是两套工作区策略

证据：`status.summary.json`、`review-diff.summary.json`、`brief-auto.md`。
同一棵 `git status` 干净的树：

| 面 | 结果 |
|---|---|
| snapshot capture | 1,828/8,458；omitted 6,630（gitignored 6,552、binary 24、too_large 47、excluded_dir 7） |
| `status --workspace` | state=changed，added 6,623，changed 0，deleted 0，needs_review=false，哈希 2,514 MB |
| `review-diff` | state=changed，added 6,623，unmapped 6,623，needs_review=true |
| `brief auto` | change brief：“6623 file(s) changed”，1 个 MEDIUM |
| `brief project` | “workspace differs … uncommitted changes” |

6,623 个 added 中 **6,595 个是 gitignored**（`target-linux` 5,698、
`baseline/logs` 544、`docs` 220、`dist` 74、`upstream-probes` 71），27 个是
快照按 binary/too_large 省略的已跟踪文件，**真正新增的未忽略文件为 0**。
根因：`drift.rs::walk_disk` 只排除 `.git`、`.archmeld`、`node_modules`、
`target`、`.venv`、`__pycache__`；不读 gitignore、不判 binary、不套 8 MiB
上限，也不排除 capture 的其他目录（所以 `target-linux` 被整棵扫）。同时
drift 自己的 coverage 写死为 `complete`。

P03 是它的语义分叉：`status` 的 `needs_review = dirty && 有 affected`，所以
只有 added 时为 false；`review-diff` 的 `needs_review = unmapped_files > 0`，
所以为 true。同一个名字，两个定义，自动化无法判断该信谁。

修复：抽一个 `workspace_policy` 同时服务 capture 和 drift；被忽略/二进制/
超大文件进 `omitted`，不进 `added`；`state` 与 `needs_review` 只看 eligible
文件；brief 的 “uncommitted changes” 只在 tracked 内容变化时出现；加一个
带 `target-linux/` 和 ignored logs 的干净树测试。

## 3. P1：会让分析结论系统性偏斜的问题

### P04 candidate 边是通用名碰撞，不是耦合

导出 34,030 条 Calls；账本 28,818 条 `static_call_candidate`。core→
`codegraph-server` 995 条、core→`codegraph` 648 条、core→harness 280、
core→vscode 269、core→memory 164；core 对这些 crate 没有任何 Cargo 依赖或
`use`。1,643 条 core→server/codegraph 中 1,630 条命中通用名：`new` 307、
`len` 240、`iter` 229、`contains` 171、`path` 129、`insert` 109、`default`
82、`read` 72、`open` 71；`new` 在 135 个文件里存在。跨语言同样发生
（core→vscode 的 269 条）。

影响：原始 fan-in 榜首是 `len`/`new`/`is_empty`/vendored `map`；`impact` 和
`path` 会穿过这些边。契约写明“candidate 不是 resolved call”是对的，但
**默认视图没有任何过滤/置信度维度**，读者仍会把它当耦合。

### P05 导出边的属性被丢弃

导出中 18,716 条 Calls 带 `resolved_by=cross_file_resolution` 且
`is_direct=true`；13,836 条无属性；911 条 `is_direct=True`；567 条
`is_direct="true"`；References 有 2,977 条 cross_file_resolution。账本中
每条 call candidate 的 conditions 只有：

```json
["codegraph-edge:Calls", "syntax-only; not a resolved call"]
```

保守地不提升为 resolved_call 是正确的；但把上游的 `resolved_by`/
`is_direct` 一起丢掉没有必要。结果是“至少被上游跨文件解析过”的候选与
“纯同名碰撞”无法区分，直接放大了 P04。

### P06 关系多重性被静默折叠

导出 51,518 条边，去重后的 `(source,target,kind)` 是 46,254；5,264 条重复
出现（Calls 5,212、References 33、Imports 19）。账本关系 46,244 条，
`bridge.coverage` 仍是 `complete`、`omitted_count=0`，报告里没有
`merged_relations`/`duplicate_edges`。也就是说：

- “28,818 条 call candidate”是 **唯一 caller/callee 对**，不是调用点；
- 一个被调用 1,000 次的函数只贡献 1 条边，fan-in/impact 没有频次；
- 5,264 次重复被折叠这件事本身不可见；
- 另有 10 条 Contains 边在去重后仍消失（导出 distinct 46,254，账本
  46,244），没有 `omitted` 记录，coverage 仍是 complete。

修复：在 bridge report 和 evidence coverage 中记录 collapsed/duplicate 边；
契约明确关系基数（pair vs occurrence）；如保留频次，加 `occurrences` 字段
而不是复制关系。

### P07 coverage 有三个口径，且互相矛盾

同一次 import：

| 位置 | 状态 | 说明 |
|---|---|---|
| snapshot | partial | 1,828/8,458，omitted 6,630，逐项列明 |
| bridge report（index.json） | complete | 13,979/13,979，unresolved 0 |
| evidence 行 | partial | 固定 `"state":"partial"`，limitations 只有“call edges are syntax candidates” |

三者都可以解释（manifest 完整 / 传输完整 / 语法证据不完整），但没有任何
地方说明差异；而且 evidence 的 limitations 不含 3 个 parser error、2,936 个
merged node、5,264 条折叠边。读者只会看到“complete”或“partial”，无法知道
缺的是什么。

### P08 语言白名单把 605 个实体变成 unknown

`bridge::language_for(path)` 只认 py/rs/ts/tsx/js/jsx/mjs/cjs/c/h/cc/cpp/
cxx/hpp/hh/hxx/java/cs/go。Kotlin（316）、TOML（273）、Bash（14）、kts（2）
因此全部 `unknown`；`status` 只报 7 种语言。导出本身对 TOML 带
`"language":"toml"`，Kotlin 节点没有 language 属性；两条路径都应在 bridge
侧用扩展名兜底。

### P09 parser error 不可归因

helper 输出 `parser_errors: {"kotlin": 3}`；账本 `diagnostics: 0`。三个
JetBrains Kotlin 源文件没有 file 实体：

```text
.../server/CodeGraphServerResolver.kt
.../server/EngineLifecycle.kt
.../mcp/McpRegistrationTest.kt
```

但这是推断，不是账本事实——没有 per-file diagnostic。P07 的 coverage 也
不提这 3 个文件。AGENTS 要求“部分覆盖必须显式”，这里只有计数，没有路径。

### P10 “confirmed” 其实是 “declared”

`brief project` 输出 `confirmed: 13,232 · candidate: 33,012`。13,232 是
declared 关系：contains 9,001 + imports 4,227 + implements 4。没有一条经过
验证；`contains` 是结构包含，`imports` 是源码声明。把 declared 叫
`confirmed` 会让 agent 把结构事实当成已验证结论。建议直接改成
`declared`/`candidate`，或在同一行给定义。

### P11 没有 test/production role 维度

1,268 个实体（11.5%）位于 `/tests/` 路径；`Store` 的 268 条 incoming 中 19
条来自测试。测试支持模块是 fan-in 大户：`crates/archmeld-core/tests/support/mod.rs`
132 条、`crates/archmeld-core/tests/benchmark.rs` 77 条。实体 payload 没有
role 字段，`archmeld_query` 也没有 role 过滤，因此：

- 架构 fan-in 混入测试 helper；
- `impact` 把测试引用算成生产影响；
- `path` 可能经过 `#[cfg(test)]`/tests 目录。

本次报告里的“core API surface”是我手工按路径前缀过滤测试后的结果；产品
本身不提供这个能力。

### P12 module 与 type/symbol 同名，且 module 没有 anchor

151 个 module 名与代码名冲突，例如 `language` 对应 50 个 symbol、`config`
42、`metrics` 38；`crate::store::Store`（module）与
`crates/archmeld-core/src/store/mod.rs::Store`（type）同名。`::Store` 查询
返回 4 个实体（external/module/symbol/type）。我第一次 MCP 探测就选错成
module，得到的 context 邻居完全不同。module 没有 anchor，不能参与 drift/
source，只适合做导入路径的占位；但查询面没有 kind 过滤/排序提示。

## 4. P2：覆盖缺口与“看不见的省略”

### P13 1,086 个捕获文件从未进入解析，原因不可见

1,828 个捕获文件 → 742 个 `files_total`。差的 1,086 个里大部分是 md/json/
yml/log（本来就不是代码），但至少 4 个是“看起来像源码/证据”的文件：

```text
4,176,254  crates/codegraph-r/tree-sitter-r-src/parser.c      file_entity 0
5,986,433  crates/codegraph-zig/tree-sitter-zig-src/parser.c  file_entity 0
2,014,697  docs/evidence/B017-model-analysis.json             file_entity 0
1,049,863  docs/evidence/e4a-.../stdout.jsonl                 file_entity 0
```

前两个是 >1 MiB 的生成 C 文件：capture 的 8 MiB 上限放行，indexer 的 1 MiB
上限跳过，helper 的 `files_skipped` 仍是 0（因为根本没被 offer）。没有
“offered/skipped-by-reason”清单，读者无法区分“语言不支持”“目录排除”
“文件太大”“解析失败”。

### P14 默认排除 `examples/`，产品自己的 bridge 不在图里

742 个解析文件里，`examples/` 下的 file 实体是 **0**。被排除的包括：

```text
crates/codegraph-server/examples/archmeld_export.rs   # 真正生成 NDJSON 的入口
crates/archmeld-core/examples/import_export.rs
crates/archmeld-core/examples/import_scip.rs
```

`archmeld-vscode/media/cytoscape.min.js` 反而以 `role=source` 进入图（P16）。
对一个自分析任务，这意味着“导出/导入怎么工作”无法从图里回答；对用户，
`examples/` 里的可执行用法也不可见。默认排除表来自上游 CodeGraph，ARCHMELD
没有覆盖机制。

### P15 trait 实现几乎没有进图

40 个语言 crate 各有 `::parse_source` 定义；导出里 `Implements` 只有 2 条，
账本 4 条 implements（还把 2 条 Kotlin `Extends` 合并进来）。`CodeParser`
trait 有类型实体，但“40 个 parser 实现同一个 trait”的架构模式在图中不可见。
这不是 ARCHMELD 的 bug（上游 resolver 没产出），但它直接影响
“plugin/registry 架构”类问题，必须写进 limitations，或由 bridge 侧做同文件
`impl Trait for` 的补充解析。

### P16 vendored 与配置节点污染排名

`archmeld-vscode/media/cytoscape.min.js`（373 KB，`role=source`）产生 104
实体、1,133 入边、219 出边，`map` 以 912 入边排全图第四。Cargo.toml 产生
273 个实体（222 type + 47 file + 4 其他），`package` 出现在 46 个文件、
`dependencies` 45 个。上游 registry 已经把 Cytoscape 标成
`vendored_asset`，但索引器不使用这个信息；`role` 分类也没有 minified/
vendored 规则。

### P17/P18 map 未自用、coverage 口径未并列

`map status` 返回 `state=absent`，仓库没有 `archmeld.map.txt`，E1/E2 的
map/entry/brief(recipe=map) 在自分析里只验证了空状态。`brief` 只打印
“1,828 files, 7 languages”，不打印“742 parsed”，也不区分 structural
`unknown` 和真实语言；snapshot partial / bridge complete / evidence partial
三个口径没有并列展示。

## 5. P1/P2：身份与可复现性

### P19 snapshot 绑定工作区字节，不是 git 内容

同一 HEAD、同一 `git diff`、同一 1,828 条路径，仅因为
`docs/archmeld-progress.md` 的 CRLF/LF 不同，得到两个 snapshot：

```text
sha256:72a51d62…  34,042,562 bytes   LF
sha256:66aaec6d…  34,042,848 bytes   CRLF
same_paths=1828, digest_differ=['docs/archmeld-progress.md']
```

内容寻址本身没有错；问题是 `status` 同时显示 `git_commit: 7b1dc280`，暗示
“这个提交的字节”。在 `core.autocrlf=true` 的机器上，fresh checkout 与旧
工作区会对同一提交产生不同 snapshot ID，证据不可互认。修复：在 git 仓库内
同时记录每个 path 的 git blob id（`git hash-object`/index），或按
`.gitattributes` 规范化；在 snapshot coverage 中写明 checkout 假设。

### P20 index 默认包含 untracked 文件

`cli.rs` 的 index 调用使用 `SnapshotOptions { include_untracked: true }`；
快照 payload 记录每个文件的 `origin`，但 `status`/`brief` 不显示 tracked/
untracked 计数。结果是“commit X 的 snapshot”可能包含本地未提交文件。本次
运行 0 个 untracked（我们临时移走了分析文档），但能力风险存在。修复：
status/brief 显示 origin 统计；CI/自分析默认 `include_untracked=false`，
需要时显式开启。

### P21 账本不绑定 CLI/helper 构建

`archmeld --version` 和 MCP `serverInfo` 只有 `0.1.0`；snapshot 的
`config_digest` build_config 只有：

```json
{"kind":"native","variant":"default","producer":"codegraph-export-v1",
 "helper":"archmeld-index","analysis_input":"snapshot_materialization"}
```

没有 helper/CodeGraph 版本或 commit。同一个 snapshot_id 可以用不同 helper
重建出不同图；本次旧 helper（11-25 16:04）与新 helper（HEAD）都得到
11,043/46,244，但 export digest 不同。F10 的旧二进制问题就是这个缺口的
直接后果。修复：`--version`/`serverInfo` 带 `GIT_HASH_SHORT`；config_digest
纳入 helper commit/version；status 展示 export 的 `producer_commit/dirty`。

### P22 关系证据是 artifact 级，不是调用点级

46,244 条关系共享 1 条 evidence（`evidence.codegraph.export.run.self.index.2`），
`source_anchor: null`，`locator: archmeld-export-v1`。导出 edge 本身也没有
line/byte（properties 只有 `is_direct`/`resolved_by`）。因此“这条 call
candidate 在哪个调用点”无法从证据回答；`source` 只能给调用者函数的声明
范围。这不是 bridge 能单独修的（上游没给位置），但契约应明确
`evidence_scope: artifact|site`，UI 也应显示“关系证据为导出级”，避免把
evidence_id 误读成行级证据。

## 6. P3：性能与可用性

### P23 每次 freshness 全量哈希

实测：`status --workspace` 3m57s、`review-diff` 2m38s、MCP `status` 2m50s、
`brief project` 2m55s、`brief auto` 6m02s（review 跑两遍：选 recipe 一次，
change_brief 一次）。被哈希的是 2.5 GB，其中 5,698 个 `target-linux` 文件
本可跳过（P02）。没有 `--no-freshness`、没有 mtime/size 缓存。MCP 的
status/brief/review 都走同一条路，agent 首次调用就要等 3 分钟。

### P24 响应体无分页/摘要

`archmeld_status` 的 MCP 响应 1,363,392 字节（含 6,623 个 added 路径）；
`review-diff` 5,614,200 字节。MCP handler 直接返回完整对象，没有
`summary_only`/`cursor`/`max_files` 参数。对 agent 的上下文预算，这是一次
“status 就把窗口打满”的风险。

### P25 helper 失败不可读

首次 index 的 helper 因 MinGW 运行时 DLL 缺失退出：

```text
{"error":"archmeld-index exited Some(-1073741511): ; snapshot … was captured but not indexed"}
```

-1073741511 = 0xC0000139（ENTRYPOINT_NOT_FOUND），但消息里没有退出码含义、
没有 helper 路径、没有 DLL 提示，stderr 为空。对首次使用者，这是一个
“快照成功、索引失败、原因空白”的死胡同。修复：解码 Windows status、打印
helper 路径/版本、提示运行时依赖或静态链接。

## 7. 根因归并

| 根因 | 涉及问题 | 说明 |
|---|---|---|
| R1 bridge 是有损规范化器 | P04–P09、P15、P22 | 语言白名单、边属性丢弃、重复边折叠、Extends→implements、parser error 不传递、无索引覆盖清单 |
| R2 工作区/覆盖策略重复 | P02、P03、P07、P23 | capture vs drift、bridge coverage vs evidence coverage、status vs review needs_review、auto brief 两次 scan |
| R3 身份不绑定 VCS/工具 | P19–P21 | 字节级 snapshot、untracked、config_digest 无 helper 版本、版本号无 build commit |
| R4 读模型缺语义维度 | P04、P10、P11、P12 | 无 resolution 置信度、无 role、module 无 anchor、关系无频次 |
| R5 context 没有优先级模型 | P01 | 种子非必需、omissions 不预算、无 per-kind 上限 |

这五条是相互放大的：R1 产生 P04 噪声 → R4 让默认查询无法过滤 → P11 再把
测试混进来 → 读者看到的 fan-in 既不可靠也不可解释。R2 让每个 agent 的第一
个 status 又慢又“脏”，R5 让最该救场的 context 返回空包。

## 8. 与 AGENTS/契约的冲突点

| 条款 | 冲突 |
|---|---|
| “未知、过期、冲突、未执行、部分覆盖都必须显式表示” | P07/P09/P13/P15：parser error、折叠边、未 offer 文件、缺失的 implements 都没有在 coverage 中显式表示 |
| “候选调用边不能提升为已解析调用” | 已满足（全部 downgrade）；但 P05 连 `resolved_by` 也丢了，属于过度保守 |
| “所有语义关系绑定来源、快照、构建配置” | P21：构建配置不含 helper 版本/commit；P19：快照绑定字节而非 git 内容 |
| “不删除失败用例以变绿” | 未违反；但 P09 让失败用例不可见，效果类似 |
| 证据必须含 cwd/argv/退出码/产物哈希 | 重命令记录在 `COMMANDS.json`（argv/退出码/时长/哈希），未全部走 `baseline/run_cmd.py`；门禁检查已进 `baseline/commands.ndjson`。这是本次方法的记录方式差异，需在 handoff 中说明 |

## 9. 修复顺序（含依赖）

**P0（先做，否则不要用这些面做发布证据）**

1. **P01**：种子元数据 + 首条 snippet 设为 mandatory；omissions 聚合/限流；
   裁剪顺序改为 relevance→page→omissions→slices→snippets→metadata；回归
   测试：`Store` 在 4,000 token 下 `selected_entities>=1`、`source_snippets>=1`。
2. **P02/P03**：抽出 capture/drift 共用的 `workspace_policy`；ignored/
   binary/too_large 进 `omitted`；`state`/`needs_review` 只看 eligible 文件；
   brief 的 “uncommitted changes” 只在 tracked 内容变化时出现；测试：
   `target-linux/` + ignored logs 的干净树必须 clean。
3. **P07**：定义唯一 coverage 语义（manifest/transport/evidence 三层分别
   命名），把 parser error、折叠边、merge 计数带进 export manifest。

**P1（发布分析结论前）**

4. **P04/P05/P06**：bridge 保留 `is_direct`/`resolved_by` 为 conditions；
   记录 duplicate/collapsed 计数；查询加 `min_confidence`/同文件/同 crate
   先验；契约升级到 1.2 并写清关系基数。
5. **P08/P09**：语言优先取 export 的 `language`，扩展名补齐 kt/kts/toml/sh/
   rb/php/swift…；parser error 带路径进 diagnostics/coverage。
6. **P10/P11**：`confirmed` 改名 `declared`；实体加 `role`（source/test/
   config/vendor），查询支持 `role` 过滤，架构视图默认排除 test。
7. **P12**：module 尽量绑定文件 anchor；查询结果按 kind 分组/提示；`::Store`
   这类歧义要有明确排序。

**P2（覆盖与身份）**

8. **P13/P14**：indexer 输出 offered/skipped-by-reason 清单；CLI 提供
   `--include-examples`/`--exclude`；自分析默认包含 examples。
9. **P15**：要么在上游 resolver 补 Implements，要么在 bridge 用同文件
   `impl Trait for` 补边并在 coverage 标明来源。
10. **P16**：`*.min.js`、vendored `media/`、Cargo.toml 配置节点默认降权/
    排除，或标 `role=vendored/config`。
11. **P19/P20/P21**：snapshot 记录 git blob id + origin 统计；config_digest
    纳入 helper commit；`--version`/`serverInfo` 带 build commit；status 展示
    producer。
12. **P22**：契约增加 `evidence_scope`，明确 artifact 级证据；如上游未来给
    调用点行号，再加 site 级。

**P3（体验）**

13. **P23**：mtime/size 缓存或 `--no-freshness`；auto brief 复用第一次
    review；MCP 明确标注耗时。
14. **P24**：status/review 增加 `summary_only`/`max_files`/分页；默认只返回
    计数 + top dirs + 前 N 条。
15. **P25**：解码 NTSTATUS、打印 helper 路径/版本、打包时静态链接或附带
    MinGW runtime。

## 10. 不建议做的事

- 不要把 candidate 提升为 resolved_call 来“让图更连通”；P04 的噪声会更大。
- 不要用 `git reset --hard`/`clean -fd` 清理 drift 噪声；先修策略，
  否则会删掉用户/证据文件。
- 不要在 brief 里把 ignored 文件从 added 里“隐藏”而不记录；应改为
  `omitted` 并保留计数。
- 不要为了 coverage=complete 而放宽 1 MiB/8 MiB 限制；应把省略原因显式化。
- 不要用记录 `record` 做回滚或 pin 修复（U5 已记录）。

## 11. 这份分析看不到的

- 3 个 Kotlin 文件是否确为 parser error：只有计数，没有 per-file 证据。
- 5,264 条折叠边是真实调用点重复，还是上游重复导出：没有行号，无法判断。
- Implements 缺失是 resolver 配置还是 parser 能力：需要上游代码级排查。
- 生产/测试边界之外的运行态影响：本次没有 import SCIP/LSP/trace，也没有
  运行测试；`impact` 仍是静态 candidate。
- release 包是否自带 MinGW runtime：本次用的是 debug 树，未验证安装包。

## 12. 与主报告的关系

主报告给出“产品面读回了什么”和 F1–F10；本文把它们拆成 P01–P25，并新增
P05/P06/P07/P10/P11/P12/P13/P14/P15/P19/P20/P22/P24/P25。修复 F1/F2 时，
应同时处理 P03/P07/P19/P21，否则“修好”的只是表面：context 仍可能被折叠边/
测试噪声污染，drift 仍可能与 snapshot 口径不一致，证据仍无法复现。
