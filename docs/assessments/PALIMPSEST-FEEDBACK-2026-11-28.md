# Palimpsest 反馈详细分析

日期：2026-11-28
分析对象：`HEAD 88dff122`（T2–T4 与公开快照合并之后）
范围：Palimpsest 对 PR #311 的六条反馈，逐条对照当前实现、证据模型、契约与测试。
方法：只读代码与已记录证据；分析阶段不执行产品命令，不修改测试期望、容差或状态字段。

实现状态（2026-11-28 同日）：ADR-062…067 已记录 brief、provenance、结构 diff、
规则例外、成本/保留与候选冻结的实现；对应测试和 `perf-review-2026-11-28-b`
基准已记录。仍未实现的是增量索引，reviewer study 仍只有协议。

## 0. 判定总表

| # | 反馈 | 判定 | 当前基础 | 主要缺口 | 建议 |
|---|---|---|---|---|---|
| 1 | change brief 准确、有界 | **属实，且比反馈更严重** | `review` 已区分 `context_only`；`brief` 已有限制框架 | `context_only` 被写成 finding；50 条上限先截断再过滤；候选边也升为 high；调用者身份不可见；无预算/省略清单 | 先修正确性，再加预算（P0-A） |
| 2 | findings 绑定精确 Git 版本 | **属实，但大部分数据已有** | snapshot 存 `git_commit`、`git_tree_digest`、`config_digest`、`environment_digest`、`exclusions_digest`、`workspace_policy`；`index_runs` 存 CLI/helper commit、dirty、配置 | review 输出只给 `base_snapshot_id`；没有 candidate commit/tree/dirty；没有 index scope 与 analyser 身份；引用没有 base/candidate 侧标签 | 补充 provenance 块（P0-B） |
| 3 | 结构化 diff：所有权与冗余路径 | **方向属实，但受证据模型限制** | 已有 snapshot-to-snapshot diff、18 种 relation kind、bounded path 查询、architecture 声明 | workspace review 没有候选图；默认 CodeGraph 导出不产生 `reads`/`writes`/`owns`；没有路径收敛/所有权 diff | 放到 snapshot diff，并把“无证据”显式标出（P1-C） |
| 4 | 项目自有规则与例外历史 | **部分已实现，缺口在例外与出处** | 6 类规则/checker、architecture revision + review 历史、`checker_id`/`verdict`/`coverage` | 无 exception 对象；规则无 `source`；无 entry point 字段；无例外扩张/移除 diff；规则只能随 architecture proposal 提交 | 加 exception + rule source + revision diff（P1-D） |
| 5 | 度量并降低连续候选的成本 | **属实；测量可行，增量索引是另一件事** | `init` 可复用 clean snapshot；CAS 去重；retention pin；`brief auto` 记录 review_ms | `index`/`refresh` 是全量 parse；review/index 无完整 timing、reuse、invalidation 输出；review 报告不持久化 | 先测量与失效原因，再单独决定是否做增量（P1-E） |
| 6 | 验证 findings 是否改善真实 review | **属实；fixture 与 reviewer study 都缺** | 已有 basic changed/added/deleted 测试与 E4a 方法论 | 没有 move、duplicate path、removed entry point、ambiguous call、stale index fixture；没有 reviewer 对照 | 先做 fixture 语义，再做小规模研究（P2-F） |

一句话结论：**第 1 条是当前代码的真实 correctness bug，应最先修；第 2 条是低成本、高价值的 provenance 补齐；第 3 条不能按字面承诺“write sites”，否则会制造证据；第 4 条不是从零开始；第 5 条的“增量更新”应单独授权；第 6 条应等语义稳定后再做。**

## 1. 证据基线

### 1.1 代码事实

- `crates/archmeld-core/src/surface/review.rs:309-315` 把“文件变了、但实体行区间未变”的实体标为 `context_only`。
- `crates/archmeld-core/src/surface/brief.rs:413-475` 遍历 `review["files"][*]["entities"]` 时**没有按 `state` 过滤**，每个实体都生成 finding；`why` 固定写成“this entity changed”。
- `crates/archmeld-core/src/surface/brief.rs:416-419` 的 50 条上限在状态过滤之前生效。
- `crates/archmeld-core/src/surface/brief.rs:428-437` 只要存在 incoming relation 就给 `high`，不区分 `candidate` / `resolved` / `declared` / `observed`。
- `crates/archmeld-core/src/surface/review.rs:426-468` 的 review 输出只含 `base_snapshot_id`、文件、实体、关系、coverage 和 verification，没有 Git/analyser provenance。
- `crates/archmeld-core/src/evidence/snapshot.rs:380-395` 的 snapshot payload 已记录 `git_commit`、`git_tree_digest`、`config_digest`、`environment_digest`、`exclusions_digest`、`workspace_policy`。
- `crates/archmeld-api/src/cli.rs:1416-1445` 的 `index_runs` 已记录 `cli_commit`、`cli_dirty`、`helper_commit`、`helper_dirty`、`export_schema`、`index_config`、`parse`、`transport`、`coverage`。
- `crates/archmeld-core/src/evidence/store/types.rs:339-358` 定义了 18 种 relation kind，包括 `owns`、`reads`、`writes`、`publishes`、`consumes`、`awaits`、`schedules`、`guarded_by`。
- 但 `crates/archmeld-core/src/evidence/bridge.rs:766-777` 默认只映射 `Calls`/`Invokes`/`Instantiates`/`Uses`、`Imports`/`ImportsFrom`、`Contains`、`Defines`、`Extends`/`Implements`、`References`；其余 edge type 只进诊断。默认 CodeGraph 导出**不产生** `reads`/`writes`/`owns`。
- `crates/archmeld-core/src/analysis/blueprint.rs:269-333` 已有 architecture rule check；`forbid_dependency`、`acyclic`、`allowed_layers`、`interface_only`、`ownership`、`custom_contract` 均有可执行 checker 或明确的 `declared_only`/`inconclusive` 结果。
- `crates/archmeld-core/src/analysis/interpret/mod.rs:163-175` 的 `ArchitectureRule` 只有 `id/kind/description/scope_ids/required/checker_id/checkability/parameters`，没有 exception 或 source。
- `docs/adr/ADR-056-agent-onboarding-and-brief.md:63-65` 明确写了 severity 映射“incoming static relation = high”，但没有说明只对 changed entity 生效；这是当前实现误用的契约空隙。

### 1.2 已记录的内部例证

`docs/evidence/e4a-2026-11-25/runs/nonlexical3-b1/stdout.jsonl` 中最后一次 `archmeld_brief`（合成 Python fixture，非 Palimpsest）的最终输出是：

| 指标 | 值 |
|---|---|
| `summary` | `5 file(s) changed; 1 affected entities; 3 candidate relation(s); verification not run` |
| `findings` | 5 |
| `context_only` findings | 3，全部 `severity=high` |
| changed findings | 1 |
| evidence-gap findings | 1 |
| 3 个 context-only 的 evidence | `file_line` + `relation`，resolution 为 `declared` |

这直接证明：**brief 的 findings 数量与 affected entities 数量不是同一口径；未变实体被写成 high finding；`why` 文字与实体的真实状态矛盾。** 该 transcript 属于私有证据，公开快照按策略省略；代码路径在当前 HEAD 仍然相同。

Palimpsest 报告的 `51 findings / 31 affected entities` 与当前实现的 `50 条实体上限 + 1 条 gap finding` 形状一致；按当前代码，51 条里最多 31 条可能是真正 changed/deleted，其余至少 19 条只能是 context-only 或 gap。这是**结构性误报**，不是措辞问题。

## 2. P0-1：change brief 的准确性与边界

### 2.1 反馈核对

| 反馈观察 | 当前代码 | 结论 |
|---|---|---|
| `context_only` 被 description 成 changed | `brief.rs:456-474` 对所有实体生成 finding；`why` 固定为“this entity changed” | 确认 |
| 51 findings 需要二次筛选 | 50 条实体上限 + 1 条 gap；且上限先于状态过滤 | 确认，且更严重 |
| 需要区分 changed / unchanged context / inferred candidate / unmapped | 只有 changed 文件内的实体 + 聚合 gap；没有 context 区；candidate 只作为 relation evidence | 确认 |
| 需要 item/byte budget 和 drill-down | 只有硬编码 50；无 `omitted` 清单；next action 只指向第一个实体且不带 snapshot | 确认 |
| 截断时保留 coverage/verification | `unknowns` 有一条“more changed entities”，但 coverage 只在 `scope.coverage`；verification 只在 finding 内部和 summary 文本 | 部分 |

### 2.2 根因

1. **状态被丢弃。** `review` 已经给出 `state=context_only`、`trigger=file_changed_but_entity_anchor_untouched`、`confidence=file_changed_entity_range_not_touched`，`change_brief` 没有使用。
2. **上限位置错误。** `findings.len() >= 50` 在 `state` 过滤之前；一个文件里大量未变实体可以挤掉后面的真正 changed 实体。更糟的是，遍历顺序是文件顺序 + entity 插入顺序，不是“changed 优先”。
3. **severity 只看 incoming，不看 resolution。** 一个 `candidate` incoming 也会把实体升为 high；这违反“候选边不是已解析调用”的产品原则。
4. **没有“上下文”概念。** 未变调用者在另一个文件时，根本不在 `review["files"]` 里；它只作为 changed 实体的 relation evidence 出现，而且 evidence 只有 relation id 和 resolution，没有调用者名字、路径、行号。
5. **gap 被聚合。** `unmapped`/`unsupported` 各生成一条 finding，`location` 为空；Palimpsest 看到的 `handoff.md` 在当前实现里不会作为独立条目出现。
6. **预算不是契约。** `archmeld_brief` 的 MCP input schema 没有 budget 参数；`BriefRequest` 也没有；默认输出无法保证“one screen”。

### 2.3 建议的契约形状

```json
{
  "findings": [
    {
      "id": "F1",
      "state": "changed|added|deleted",
      "severity": "high|medium|low|info",
      "confidence": "static_line_overlap|no_base_entity_mapping|...",
      "entity": {"entity_id": "...", "qualified_name": "...", "path": "...", "line": 12},
      "relations": [
        {"relation_id": "...", "kind": "declares", "resolution": "declared", "other": {...}}
      ],
      "next_action": {"tool": "archmeld_source", "args": {"entity_id": "...", "snapshot_id": "..."}}
    }
  ],
  "context": [
    {
      "state": "unchanged_context",
      "reason": "file_changed_entity_anchor_untouched|relation_target_changed",
      "entity": {"entity_id": "...", "qualified_name": "...", "path": "...", "line": 3},
      "relation": {"relation_id": "...", "kind": "declares", "resolution": "declared"},
      "side": "base"
    }
  ],
  "gaps": [
    {
      "kind": "unmapped|unsupported|stale|truncated",
      "path": "handoff.md",
      "reason": "no base entity mapping",
      "next_action": {"tool": "archmeld_review", "args": {"base_snapshot_id": "...", "path": "handoff.md"}}
    }
  ],
  "relations": {
    "confirmed": 7,
    "candidate": 1222,
    "items": [{"kind": "static_call_candidate", "resolution": "candidate", "count": 1200}]
  },
  "budget": {"max_findings": 20, "max_context": 10, "max_gaps": 10, "returned": 31, "omitted": 12, "truncated": true},
  "coverage": {"state": "partial", "limitations": ["..."]},
  "verification": {"state": "not_run", "reason": "..."},
  "next_actions": [{"tool": "archmeld_review", "args": {"base_snapshot_id": "...", "detail": "full"}}]
}
```

关键规则：

- `findings` 只允许 `changed|added|deleted`。`context_only` 永不进入 `findings`；保留 `findings` 字段名是为了不破坏现有消费者，语义收窄为“changed findings”。
- `context` 与 `findings` 分开计数、分开截断；context 不计入 findings 总数。
- `gaps` 每个文件一条（可截断），带路径；不再用一条聚合 finding 代表全部 unmapped。
- `relations` 只做聚合与 top-N，不把 1,222 条候选关系塞进 header；候选数不是“发现数”。
- severity：incoming `resolved|declared|observed` 才是 high；`candidate` 最多 medium/info，并带 `confidence=candidate`；added medium、deleted low、其余 info 保持不变（需要 ADR-056 补一句“只对 changed 实体”）。
- 预算用**条数**而不是字节，验收允许 item budget；默认值建议 `findings=20, context=10, gaps=10`，可由 CLI/MCP 参数降低。若坚持字节预算，另加 `max_bytes`，但先不要同时做两套。
- `counts.findings` 返回**截断前**的 changed finding 总数；`budget.returned/omitted` 才表示本次返回量。否则调用方无法知道是否被截断。
- 每个 `next_action` 必须带 `snapshot_id`。当前 `brief` 的 next action 只带 `entity_id`，MCP 默认会取 latest snapshot；如果 brief 之后发生 refresh，drill-down 会落到另一版图。
- `coverage` 和 `verification` 提升到顶层，不依赖调用方从 summary 文字里解析。

### 2.4 验收映射

| Palimpsest 验收 | 本仓库 fixture |
|---|---|
| 改一个函数、调用者不动，brief 把调用者标为 unchanged context 并引用关系 | `src/a.py::a` 改动；`src/b.py::b` 调用 `a`；断言 `context` 含 `b`，relation id 指向 `a` |
| 包含 unmapped 文件 | `handoff.md` 无 base anchor；断言 `gaps[*].path == "handoff.md"` |
| 小预算不超限，报告 omitted，保留 coverage/verification | `max_findings=1`；断言 `budget.omitted > 0`、`coverage`/`verification` 仍在 |
| drill-down 从同一 snapshot 打开 | 断言每个 next action 的 `snapshot_id` 等于 brief 的 `scope.snapshot_id` |
| 回归：context-only 不能挤掉 changed | 构造 60 个未变实体 + 1 个 changed 实体；断言 changed 仍在 `findings` 中 |

### 2.5 成本与风险

- 代码改动集中在 `surface/brief.rs`、`BriefRequest`、CLI/MCP 参数、MCP input schema 和测试；预计 2–3 天。
- 需要 ADR-056 的修订或新 ADR：明确 severity 只对 changed 实体生效；候选边不升 high。
- 默认 findings 从 50 降到 20 会改变 agent 看到的数量，但这是产品承诺“one screen”的必要条件；保留 `detail=full` 或 `max_findings=50` 作为显式逃生口。
- 不要把 `context` 也塞进 `findings` 来“兼容旧消费者”；旧消费者应当读 `findings`（语义收窄为 changed findings），这是 breaking output change，需要版本号或明确的兼容字段。

## 3. P0-2：findings 绑定精确 Git 版本

### 3.1 反馈核对

| 反馈观察 | 当前实现 | 结论 |
|---|---|---|
| review 结果只有 base snapshot ID | `review.rs:426-468` 确认 | 确认 |
| 宿主另记 candidate SHA 才能追溯 | CLI/HTTP 都没有 candidate commit 输出 | 确认 |
| 需要 base/candidate commit、tree、snapshot、analyser build、config digest、index scope | snapshot 有前 3 项和 config/environment/exclusions/policy；index_runs 有 build/config；review 不输出 | 数据大多已有，输出缺失 |
| dirty workspace 要显式标记 | review 只有 `state=changed`；不能区分 dirty 与换了一个 commit | 确认 |
| 引用要标 base/candidate 侧 | review 的 entity 引用是 base anchor；line change 有 old/new；没有统一 side 标签 | 部分 |
| 两个 commit 同 tree 要保留两个身份并暴露 tree 等价 | `diff_snapshots` 在 `state=unchanged` 时提前返回，只给两个 snapshot id，不给 tree | 确认 |

### 3.2 现成数据与命名

- `snapshot.git_commit`：capture 时 `git rev-parse HEAD`；非 git 为 null。
- `snapshot.git_tree_digest`：**不是 Git tree OID**，而是 `path + (git_blob | working:blob_digest)` 的规范化 digest（`snapshot.rs:280-301`）。它是“Git blob 或工作区内容”的混合身份：干净 tracked 文件用 Git blob，dirty/untracked 文件用 working digest。两个内容相同的 snapshot，如果一个是干净 captured、另一个是在 dirty 状态下 captured，`git_tree_digest` 可能不同；因此它不能单独作为内容等价判据。
- 真正的“评审字节”等价应使用**纯内容 digest**：`path + file.blob_digest`（snapshot 的每个 file 都有 `blob_digest`），本报告称为 `content_tree_digest`。候选工作区可以用 drift scan 的 `path -> current_digest` 算出同一算法。
- `git rev-parse <commit>^{tree}` 得到的是该 commit 的 Git tree，只覆盖 tracked 内容；如果 snapshot 包含 untracked 文件或 dirty tracked 文件，它不等于 snapshot 的字节。输出里只能叫 `head_tree_oid`/`commit_tree_oid`，不能叫“snapshot tree”。
- `snapshot.config_digest` / `environment_digest` / `exclusions_digest` / `workspace_policy`：足以表达构建配置和索引范围，但 manifest 本身没有存进 snapshot，只有 digest。
- `index_runs`：`cli_commit`、`cli_dirty`、`helper_commit`、`helper_dirty`、`export_schema`、`index_config`、`parse`、`transport`、`coverage`。这比 snapshot 的 build digest 更适合回答“哪个 analyser build 产生了这张图”。
- `review` 的 per-file `base_digest`/`current_digest` 已经能证明文件内容；缺的是顶层 candidate 身份和 side 标签。

### 3.3 建议输出

```json
{
  "provenance": {
    "base": {
      "snapshot_id": "sha256:...",
      "git_commit": "...",
      "head_tree_oid": "...",
      "content_tree_digest": "sha256:...",
      "git_tree_digest": "sha256:...",
      "config_digest": "sha256:...",
      "environment_digest": "sha256:...",
      "exclusions_digest": "sha256:...",
      "workspace_policy": {"include_untracked": true, "max_file_bytes": 8388608, "excluded_dirs": ["..."], "max_depth": 64},
      "analyser": {
        "cli_commit": "...", "cli_dirty": false,
        "helper_commit": "...", "helper_dirty": false,
        "export_schema": "archmeld-export-v1",
        "index_config": {"max_files": 5000, "include_dirs": [], "exclude_dirs": []}
      }
    },
    "candidate": {
      "kind": "workspace",
      "git_commit": "...",
      "head_tree_oid": "...",
      "commit_tree_oid": "...",
      "dirty": true,
      "dirty_paths": 3,
      "content_tree_digest": "sha256:...",
      "durable": false
    },
    "tree_equivalent": false,
    "durability": "base_snapshot_immutable; candidate_workspace_not_frozen"
  }
}
```

规则：

- `base.head_tree_oid` 可以用 `git rev-parse <git_commit>^{tree}` 在 review 时查询；它只表示该 commit 的 tracked tree，不代表 snapshot 的完整字节。snapshot 包含 untracked 或 dirty 文件时，必须在 `covers`/`limitations` 里说明，不能把它当“snapshot tree”。
- `candidate.git_commit` 是 `HEAD`；`candidate.dirty` 来自 `git status --porcelain`。dirty 时 `candidate.commit_tree_oid` 只作为 `HEAD` 的 tree 记录，不代表评审字节；真正的评审字节用 `content_tree_digest`。
- `base.content_tree_digest` 与 `candidate.content_tree_digest` 都用 `path + blob_digest`（候选用 current_digest）计算；两者相等则 `tree_equivalent=true`，即使 commit 不同。若两边都是干净 tracked、且 `head_tree_oid` 相等，也可作为 Git 层面的等价证据；但优先报 content digest。
- `base.analyser` 来自 `latest_index_run`；缺失时 `analyser=null` 并加 coverage limitation，不能默认为“当前 build”。
- 每个 file/entity/line-change 引用加 `side: "base"|"candidate"|"both"`；base entity 用 `base_snapshot_id`，candidate 行号用当前字节。
- `durability` 明确：base 是冻结 snapshot；candidate 是 workspace 读取，默认不冻结。若需要“引用永久可解析”，必须另做 candidate snapshot（见 §10）。

### 3.4 `diff_snapshots` 的两 commit 验收

当前 `review.rs:492-500` 在 base==new 时提前返回；不同 commit、同 tree 会走完整比较，但输出没有 Git 身份。建议：

- 在 `diff_snapshots` 返回中加 `base`/`candidate` provenance（从 snapshot payload 读）。
- 优先比较 `content_tree_digest`；相同则 `tree_equivalent=true`，并保留两个 `git_commit`。两边都干净 tracked 时，再比较 `head_tree_oid` 作为 Git 层证据；不要只比较 `git_tree_digest`（它的 git_blob/working 混合身份可能不同）。
- 即使 `state=unchanged`，也保留 provenance；不要因为文件/实体/关系都没变就丢掉身份。
- 增加 `identity_changes`：`config_digest`、`environment_digest`、`exclusions_digest`、`workspace_policy`、`analyser` 的变化原因。这同时服务第 5 条。

### 3.5 验收映射与成本

| 验收 | 测试 |
|---|---|
| rebase 后两个 commit 同 tree | 两个 snapshot 的 `content_tree_digest` 相同；断言两个 `git_commit` 都在，`tree_equivalent=true` |
| dirty 与 HEAD 可区分 | 修改 tracked 文件；断言 `candidate.dirty=true`、`commit_tree_oid` 只代表 HEAD、`content_tree_digest` 变化 |
| 每条引用可解析 | base entity 引用带 `base_snapshot_id`；candidate line 引用带 `current_digest`；测试断言 side |
| analyser/config/scope 可见 | `provenance.base.analyser` 与 `index_config` 存在；无 index_run 时 limitation |

成本 3–4 天；风险主要是 Git 命令的跨平台/缺仓库容错。当前 `snapshot.rs` 已有 `git_sets`/`git_dirty_paths` 等私有 helper，建议抽成可复用、可选失败的 `workspace_git_identity`，不要在每个输出点直接 `Command::new("git")`。

## 4. P1-3：结构化 diff、所有权与冗余路径

### 4.1 反馈核对

| 反馈要求 | 当前实现 | 结论 |
|---|---|---|
| 显示新增/删除的跨层依赖、入口、adapter、write/recovery path | `diff_snapshots` 只按 path/entity/relation id 列 added/deleted/changed；review 只做 base graph impact | 缺口 |
| 高亮两条路径拥有同一 transition | 无路径收敛分析；`query path` 只给最短路径 | 缺口 |
| 显示简化（删入口、调用者改用共享实现） | 无 | 缺口 |
| 引用代码并把 inferred ownership 标为 hypothesis | review 有 `resolution`/`confidence`，但没有 ownership/path 结构 | 部分基础 |
| #311 要追 dispatch ownership、restart 后读取、takeover、settlement 入口 | 默认图只有 calls/imports/contains/declares/implements/references | **证据模型不支撑** |

### 4.2 关键限制：`reads`/`writes`/`owns` 没有生产者

`types.rs` 定义了 18 种 relation kind，但 `bridge.rs` 的默认导出映射只产生：

- `static_call_candidate`：`Calls` / `Invokes` / `Instantiates` / `Uses`
- `imports`：`Imports` / `ImportsFrom`
- `contains`：`Contains`
- `declares`：`Defines`
- `implements`：`Extends` / `Implements`
- `references`：`References`

其余 edge type 进 `unmapped_edge_type` 诊断。这意味着：

- 默认 CodeGraph 静态图**没有**“谁写了 dispatch 状态”“谁在 restart 后读它”“settlement 写到哪里”的边。
- `owns` 只可能来自 architecture 的 `owned_state`/`owned_resources` 声明，或手工导入；`writes`/`reads` 只可能来自 runtime/scip 等外部证据，当前默认路径没有。
- 因此，若按字面实现“surface both paths and their write sites”，产品会在没有 `writes` 证据时把 calls 路径当成写入路径，或把声明 ownership 当成实际写入。这正是 AGENTS.md 禁止的“候选/推断冒充已确认”。

**结论：第 3 条必须先缩小承诺。** 可交付的是“关系 diff + 有界路径假设 + 声明所有权变化”，不是“自动所有权/写入路径检测”。

### 4.3 建议的交付边界

放在 **snapshot-to-snapshot** 侧，而不是 workspace review：

- `review-diff --candidate-snapshot <id>`（或 `refresh` 的 `structural` 块）读取两个已索引 snapshot。
- workspace-only review 没有候选图，无法知道新增 calls/relations；不要假装它能做结构化 diff。

建议输出：

```json
{
  "structural": {
    "files": {"moved": [{"from": "a.py", "to": "b.py", "digest": "..."}], "added": [], "deleted": [], "changed": []},
    "dependencies": {
      "added": [{"source": "...", "target": "...", "kind": "static_call_candidate", "resolution": "candidate", "evidence": ["..."], "hypothesis": true}],
      "removed": []
    },
    "entry_points": {"added": [], "removed": [], "basis": "declared"},
    "ownership": {"added": [], "removed": [], "basis": "declared|observed", "coverage": "none|partial|complete"},
    "paths": {
      "convergence": [{"target": "...", "paths": [[...], [...]], "basis": "static_call_candidate", "hypothesis": true}],
      "coverage": "partial",
      "limitations": ["reads/writes evidence is absent; paths are call/import only"]
    }
  }
}
```

实现要点：

1. **文件移动**：`diff_snapshots` 目前按 path 比较；先按 `blob_digest` 匹配 deleted/added，输出 `moved`，避免把 move 误报为“删旧实现 + 新增实现”。这直接覆盖第 6 条 fixture 的一部分。
2. **关系 diff 的稳定键**：relation id 在实体内容变化后会变；当前 `diff_snapshots` 按 id 比较会产生 delete+add 噪声。用 `(kind, source qualified_name, target qualified_name, resolution)` 作为结构键，再保留 id 作为证据引用。
3. **路径收敛**：从 changed/added 实体或声明的 entry point 出发，在候选图上做 depth ≤ 4、node ≤ 2000 的有界 DFS，按 endpoint 分组；同一 endpoint 有 ≥2 条 node-disjoint 路径时报告 convergence。每条边带 `kind`/`resolution`/evidence；只要有一条 `candidate`，整体 `hypothesis=true`。不要把它叫“ownership”。
4. **所有权**：只读 `owns`/`writes` 或 architecture 声明；没有证据时 `coverage: "none"` + limitation，不输出空数组冒充“没有写入”。
5. **入口点**：优先用 architecture 声明的 components/entry points；没有声明时不要用“无 incoming call”猜测，除非明确标 `basis:"heuristic"` 且 `hypothesis:true`。
6. **简化**：删除的 entry point + 调用者指向共享实现，可用 removed relation + added relation 的 endpoint 变化表达；这是“假设”，不是语义证明。

### 4.4 验收映射与成本

| Palimpsest 验收 | 本仓库可行的 fixture |
|---|---|
| 引入第二条 acyclic settlement path，显示两条路径和 write sites | 两个 snapshot，synthetic `Calls` 边形成两条路径；`write sites` 只能来自 runtime relation 或 architecture 声明。若没有，验收必须改为“write evidence unavailable” |
| 删除旧路径、调用者迁移，报告收敛和未解决调用者 | 两个 snapshot：删除旧 calls 边/入口，新增共享实现边；断言 removed/added 和 unresolved callers |
| 引用代码并标 hypothesis | 每条 path 边带 evidence；candidate 边 `hypothesis=true` |

成本 5–7 天；若要求真实 `writes`/`reads`，需要新增语言/运行时候选证据源，成本远高于 diff，且必须单独 ADR。**这是第 3 条最大的范围风险。**

## 5. P1-4：项目自有规则与例外历史

### 5.1 当前已有什么

- `Architecture` 是项目级、revisioned、可 review 的记录；`rules` 随 architecture revision 保存。
- `check_architecture_rules` 返回 `registry`、`results`、`blocking_rules`、`state`；每个 result 有 `rule_id`、`kind`、`required`、`checker_id`、`checkability`、`verdict`、`passed`、`blocks_acceptance`、`reason`、`coverage`、`violations`。
- 已有 checker：`forbid_dependency` + `ledger.dependency_path.v1`、`acyclic`、`allowed_layers`、`interface_only`、`ownership`、`custom_contract`。
- `coverage=partial` 时返回 `no_violation_found` 而不是 `passed`；未注册 checker / declared_only 会阻塞 required rule。这已经满足“区分显式规则违反、图事实、工具推断”的大部分要求。

### 5.2 缺口

| 反馈要求 | 当前 | 缺口 |
|---|---|---|
| 项目提供/引用 versioned layer rules | architecture revision 里有 rules；没有仓库文件/引用 | 规则只能随 proposal JSON 提交，不能从版本化文件加载/引用 |
| allowed imports | `forbid_dependency` 可表达 | 可用 |
| protected entry points | 无字段 | 需要定义/新增 |
| accepted exceptions | 无 | 需要 exception 对象、匹配语义、历史 |
| 区分 violation 与 graph fact/tool inference | `checker_id`/`verdict`/`coverage` 已有 | 基本满足 |
| 例外 introduction version/rationale/scope | 无 | 需要 `introduced_revision`/`reason`/`scope_ids` |
| 例外扩张/移除可见 | 无 | 需要 revision diff |
| 引用 exact rule 和 source location | 只有 rule_id，没有 rule source | 需要 `source {path,line,digest}` |

### 5.3 建议的最小扩展

1. `ArchitectureRule` 增加可选：
   - `source: {path, line, digest}`：规则在项目中的出处；路径必须是 workspace-relative，校验 traversal。
   - `exceptions: [{id, scope_ids, reason, introduced_revision, expires_revision?, source?}]`。
2. `check_architecture_rules`：
   - 命中 exception 的 violation 进 `excepted`，带 exception id/reason/introduced revision；不进入 `violations`，但仍出现在结果里。
   - 未被任何 violation 使用的 exception 进 `unused_exceptions`，明确“可移除”。
   - required rule 被 exception 覆盖时，`blocks_acceptance=false` 但 `state="excepted"`，不能伪装成 `passed`。
   - `exception_changes` 与上一 revision 比较：`added`/`removed`/`expanded`/`narrowed`；这就是“何时扩张、何时可移除”的可审计答案。
3. 规则来源：优先在 architecture proposal 中带 `source`，由项目仓库的文件生成/校验；不要默认读取任意路径。若需要“项目引用一个版本化规则文件”，再加 `archmeld architecture check --file <path>`，并限制在注册 workspace 内。
4. protected entry points：建议先不加新 checker；用 `interface_only`/`forbid_dependency` + component scope 表达。若确有需求，再加 `entry_points` 字段和 checker，避免一次扩张规则语言。

### 5.4 验收映射

| 验收 | 测试 |
|---|---|
| 新 forbidden dependency + 既有 exception | 两个 revision：rule `forbid A->B`，exception 覆盖一条路径；新增第二条路径；断言新 violation 引用 rule/source，旧路径进 excepted |
| exception 不变 | 断言 `excepted` 的 id/reason 与上一 revision 相同 |
| 扩张/移除 | 删除 exception 后 `unused_exceptions`/`exception_changes.removed`；扩大 scope 后 `expanded` |
| exact rule/source | 断言 violation 带 `rule_id` 和 `source.path/line` |

成本 3–4 天（含 contract patch + ADR + tests）；文件式规则命令再加 2 天。风险：给 required rule 加 exception 会改变“required 必阻塞”的语义，必须在 ADR 中写清楚“excepted 不等于 passed”。

## 6. P1-5：连续候选的成本、复用与失效

### 6.1 当前事实

- `docs/adr/ADR-036-offline-index-and-report-cli.md:48` 明确“Indexing is a full structural parse per invocation; incremental state is not implemented.”
- `init` 会尝试 `existing_clean_index`（`cli.rs:835-883`，调用点在 `cli.rs:961`）：workspace 对 latest snapshot 完全 clean 时复用；否则全量 capture + parse + import。`index`/`refresh` 不经过这个复用路径。
- `refresh`（`cli.rs:764-828`）总是调用 `run_index_with`，即全量 parse；它只做 snapshot-to-snapshot diff，不做增量索引。
- `review_workspace_diff` 会扫描整个 eligible workspace；`brief auto` 记录 `review_ms`，CLI `review-diff` 不记录 timing。
- snapshot/CAS 是 content-addressed；retention 只按 snapshot 计数并保护引用；review 输出不持久化，只有 drift report 持久化。
- 没有 `reuse`/`invalidation` 计数；没有“为什么这次证据失效”的结构化输出。

### 6.2 可先做的测量

1. 在 `index`、`refresh`、`review-diff` 输出加 `timings`：capture、helper、import、diff、total；单位 ms。
2. 在 `refresh`/`diff_snapshots` 加 `reuse`：
   - `files_unchanged_vs_base`
   - `blobs_reused` / `blobs_new`（按 digest 对比 base snapshot）
   - `snapshot_reused`（`existing_clean_index`）
   - `parse` 仍是 full：明确 `index_mode: "full_parse"`，不要写成 incremental。
3. 加 `invalidation`：
   - `config_digest` 变化 → `config_changed`
   - `environment_digest` 变化 → `environment_changed`
   - `exclusions_digest`/`workspace_policy` 变化 → `scope_changed`
   - `index_run.cli_commit/helper_commit` 变化 → `analyser_changed`
   - `content_tree_digest`/`git_tree_digest` 变化 → `tree_changed`
   这直接回答“为什么 affected evidence 失效”。
4. 基准：3 个连续候选（同一 baseline），分别测 cold index、same-baseline review、refresh；记录 elapsed、磁盘增量、reuse、invalidation。用 `baseline/run_cmd.py` 记录，生成 `docs/evidence/perf-review-YYYY-MM-DD/`。
5. 持久化与 pin：如果要满足“reclaiming temporary artifacts leaves pinned review evidence readable”，需要把 review/structural diff 结果存成 `review_reports`（base/candidate ids、output digest、timings），并 pin 两个 snapshot。现有 `retention_pins` 可复用，但需要新的 kind/route。否则只能说“snapshot 可重放”，不能说“review 证据被 pin”。

### 6.3 增量索引是另一件事

Palimpsest 的验收写了“incremental updates”。当前实现没有：

- CodeGraph helper 每次全量 parse；没有 per-file delta export。
- 账本按 snapshot 全量发布；没有 entity/relation 级增量事务。
- 要做真正的增量，需要新的 exporter/contract、变更文件检测、删除传播、跨文件引用失效、部分覆盖表示，成本估计 10–15 天，且会改变证据模型。

**建议：把第 5 条拆成 5a（测量/失效/pin）和 5b（增量索引，单独 ADR/授权）。** 不要把 5b 混进“补 timing”里，否则会得到一个看似满足、实际全量重算的假增量。

### 6.4 验收映射与成本

| 验收 | 5a 是否满足 | 说明 |
|---|---|---|
| 三个候选的时间/磁盘 delta | 是 | 用 fixture，不用 Palimpsest 数据 |
| unchanged rerun 报 reuse | 是 | `existing_clean_index` + blob reuse |
| analyser/config/scope 变化报 invalidation | 是 | `invalidation` 块 |
| 回收临时产物后 pinned review 可读 | 需要新增 review report/pin | 只 pin snapshot 不够 |
| incremental update 报 reuse | **否** | 全量 parse；需 5b |

成本：5a 3–4 天；review report 持久化 + pin 2–3 天；5b 单独 10–15 天。

## 7. P2-6：fixture 与 reviewer study

### 7.1 现有测试覆盖

- `crates/archmeld-core/tests/diff_review.rs`：一个主测试，覆盖 changed/added/deleted、entity state、impact、coverage partial、snapshot diff；没有 move、duplicate path、removed entry point、ambiguous candidate、stale index。
- `crates/archmeld-core/tests/brief.rs`：7 个测试，覆盖 project/change/map、candidate evidence、auto review 一次；没有 `context_only` 不得成为 finding 的断言，也没有预算/省略。
- `crates/archmeld-core/tests/blueprint_rules.rs`：规则 checker 覆盖较好；没有 exception/history。
- E4a 是 map/no-map 的 no-go，不是 review brief 的对照研究；不能拿来证明“brief 有用/无用”。

### 7.2 建议 fixture 矩阵

| Fixture | 期望 |
|---|---|
| 文件移动（内容相同，路径变化） | `moved`，不是 deleted+added；调用者不变 |
| 改函数、调用者不动 | changed 有函数；context 有调用者；relation 引用正确 |
| 两条 acyclic settlement 路径 | structural paths 显示两条；candidate 边标 hypothesis；无 writes 证据时明确 unavailable |
| 删除旧入口、调用者改用共享实现 | removed entry point；added shared relation；unresolved callers 单列 |
| 歧义调用（同名/多候选） | 保持 `candidate`；不升 high；brief/graph 都标 resolution |
| 部分覆盖/过期快照 | coverage partial、stale=1、unknowns 保留；不因截断消失 |
| 大量 context-only + 少量 changed | changed 不被 50 条上限挤掉 |

### 7.3 Reviewer study

在第 1–4 条语义稳定后做，不要现在做。建议：

- 任务：同一组维护任务（file move、untouched caller、duplicate path、removed entry point、stale index）。
- 两臂：Git diff alone vs Git diff + bounded ARCHMELD brief。
- 指标：定位正确 owner 的时间、遗漏/无依据结论数、结论能否从 cited source/project tests 复现。
- 样本小、描述性统计；不要声称因果或统计显著。
- 复用 E4a 的记录格式，但不要把 E4a 的 no-go 外推到 review brief。

成本：fixture 3–5 天；study 5–10 天（取决于 reviewer 数量）。

## 8. 横向问题

### 8.1 “one screen”还没有成为契约

`brief` 默认最多 50 findings，`render_markdown` 会把每个 finding 和 evidence 全部打印；这既不是 one screen，也不是有界。第 1 条修复应同时定义：

- `findings`/`context`/`gaps` 三个独立预算；
- `budget.returned/omitted/truncated`；
- markdown 显示 “showing X of Y”，不静默截断；
- MCP input schema 增加预算参数（通过 `contracts/archmeld` patch，不直接改冻结包）。

### 8.2 snapshot 身份 ≠ candidate 身份

snapshot id 是内容身份，包含 `git_commit`、文件、coverage、config/environment/exclusions、workspace_policy；`created_at` 被排除。两个 commit 同 tree 会有不同 snapshot id，但 `content_tree_digest`（以及干净 tracked 情况下的 Git tree）相同。review 只有 base snapshot id，不能表达 candidate；`diff_snapshots` 又在 unchanged 时丢掉身份。第 2 条修复应统一用 `provenance` 块，不要在多个输出里各拼一套字段。

### 8.3 证据词汇大于证据生产

`types.rs` 有 18 种 relation kind，但默认 bridge 只生产 6 种。第 3 条如果按“读取/写入/所有权”字面实现，会系统性高估证据。应在输出里区分：

- `declared`：architecture/规则声明
- `observed`：runtime/scip 证据
- `resolved`：语义解析
- `candidate`：静态候选
- `unavailable`：没有生产者

并在 coverage 里写清楚“没有 reads/writes 证据”，而不是返回空数组。

### 8.4 review 的候选字节不持久

`review_workspace_diff` 只读 workspace，不写 CAS；base 引用可重放，candidate 行号在 workspace 改变后不可重放。第 2 条验收里的“每条引用可解析”需要明确：

- base：可解析（snapshot CAS）。
- candidate：只有 `current_digest` 和当时 workspace；默认 `durable=false`。
- 若要 durable candidate，新增 `--freeze` 捕获 candidate snapshot，或把 review report 连同候选字节存 CAS；这是产品/权限决策，不应偷偷做。

### 8.5 性能

Palimpsest 517 文件 review 7.7s；本仓库自分析曾出现 review 2m38s（2.5GB 扫描）。第 5 条测量应同时报告“eligible 文件数/字节”，不要只报秒数；否则不同仓库的数字不可比。Windows 路径/大小写/杀毒软件影响也应记录在 host 块。

## 9. 推荐顺序

| 顺序 | 工作 | 依赖 | 预估 |
|---|---|---|---|
| A | brief 正确性 + 预算 + 测试 | 无 | 2–3 天 |
| B | review/diff provenance + candidate identity + tree equivalence | 无（可与 A 并行研究，但单写入者串行） | 3–4 天 |
| C | snapshot-to-snapshot 结构化 diff（move/edge/path hypothesis） | B 的 candidate 身份/provenance | 5–7 天 |
| D | architecture exceptions + rule source + revision diff | 无 | 3–4 天 |
| E | timing/reuse/invalidation + review report pin | B/C | 4–6 天 |
| F | fixture suite + reviewer study | A/B/C/D | 3–5 天 + 5–10 天 |

建议先交付 **A+B**：它们直接回应 Palimpsest 的两个 P0，且不需要新证据源。C 必须等 B 的 candidate 身份稳定，否则 structural diff 无法引用“哪个候选”。D 可以独立推进。E 的增量部分不要与 5a 混在一起。F 最后做，避免用未稳定的语义训练 reviewer。

## 10. 需要决策的点

1. **candidate review 是否允许冻结？**
   - 推荐默认：保持 read-only，输出 `durable=false`，只保证 base 可重放。
   - 后续可选：`review --freeze` 捕获 candidate snapshot，换取可重放的 candidate 引用；代价是 review 会写 ledger，需新 ADR。
2. **第 3 条是否接受“write/read evidence unavailable”？**
   - 推荐：接受。先做 calls/imports 路径 + declared ownership；不伪造 writes。
   - 若必须真实 write sites，需先新增 `reads`/`writes` 证据源（语言 adapter 或 runtime），这是独立项目。
3. **第 5 条是否包含增量索引？**
   - 推荐：5a 测量/失效/pin 先做；5b 增量单独 ADR，不放进本轮验收。
4. **brief 默认预算取多少？**
   - 推荐 `findings=20, context=10, gaps=10`；`detail=full` 或 `max_findings=50` 作为显式逃生口。
5. **项目规则用 architecture revision 还是仓库文件？**
   - 推荐：先加 `exceptions` + `source` 到 architecture revision；文件式加载作为后续命令，不默认读取任意路径。
6. **“protected entry points”是否现在做？**
   - 推荐：先复用 `interface_only`/`forbid_dependency` + component scope；没有具体需求前不扩规则语言。

## 11. 明确不做

> 本节是分析阶段（`88dff122`，实现前）的边界记录。后续 ADR-067 明确允许
> `review-diff --freeze --persist` 显式持久化 changed/added 候选字节；
> 这不是默认行为，也不改变 snapshot/latest。其余条目仍然有效。

- 不把 `context_only` 重新塞进 `findings` 来“保持数量”。
- 不把 `candidate` relation 升为 high 或写成 confirmed call。
- 不根据名字相似、调用路径或 architecture 声明推断实际 write/read ownership。
- 不在 workspace review 里假装有候选图；结构化 diff 只对两个已索引 snapshot。
- 不把全量 refresh 重新命名为 incremental。
- 不为“看起来满足验收”默认持久化 candidate 字节或修改 source/history；
  只有显式 `--freeze --persist` 才把 changed/added 字节写入 CAS 并 pin。
- 不直接改冻结包 `docs/archmeld/contracts`；新参数走 `contracts/archmeld` patch + 新 ADR。

## 12. 证据索引

| 主题 | 位置 |
|---|---|
| context_only 产生 | `crates/archmeld-core/src/surface/review.rs:309-315` |
| brief 遍历/上限/severity | `crates/archmeld-core/src/surface/brief.rs:413-475` |
| brief gap/unknown/summary | `crates/archmeld-core/src/surface/brief.rs:481-548` |
| review 输出 | `crates/archmeld-core/src/surface/review.rs:426-468` |
| snapshot 身份字段 | `crates/archmeld-core/src/evidence/snapshot.rs:280-301`, `:380-395` |
| index run 身份 | `crates/archmeld-api/src/cli.rs:1416-1445` |
| relation kind | `crates/archmeld-core/src/evidence/store/types.rs:339-358` |
| bridge 映射 | `crates/archmeld-core/src/evidence/bridge.rs:766-777` |
| architecture rule check | `crates/archmeld-core/src/analysis/blueprint.rs:269-333` |
| architecture rule 结构 | `crates/archmeld-core/src/analysis/interpret/mod.rs:163-175` |
| severity 契约 | `docs/adr/ADR-056-agent-onboarding-and-brief.md:63-65` |
| full parse 声明 | `docs/adr/ADR-036-offline-index-and-report-cli.md:48` |
| diff review 测试 | `crates/archmeld-core/tests/diff_review.rs:135-246` |
| brief 测试 | `crates/archmeld-core/tests/brief.rs:125-161` |
| MCP review 分页 | `crates/archmeld-api/src/mcp.rs:803-844` |
| retention | `crates/archmeld-core/src/evidence/retention.rs:49-73`, `:109-210` |
