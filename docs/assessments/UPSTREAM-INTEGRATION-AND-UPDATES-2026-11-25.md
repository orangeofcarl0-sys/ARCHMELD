# 上游集成与更新策略

分析日期：2026-11-25。范围：ARCHMELD 仓库当前集成的 CodeGraph、
CodeBoarding、SCIP、LSP 工具、Perfetto/Tracy、AOCI-CODE、Cytoscape.js
以及工具链。回答两个问题：

1. 现在是怎么处理这些集成的？是不是统一兼容层？
2. 怎样把上游更新变成可重复、低风险、可审计的流程？

## 0. 结论

1. **不是统一的兼容层，而是分层边界。** 只有 CodeGraph 是代码级薄分叉；
   其余是“外部工件导入”“进程/协议适配”“只读文本互操作”“vendored
   资产”和“工具链 pin”。把全部上游塞进一个兼容层反而会增加耦合。
2. **当前更新机制是“pin + 手工 drill + 证据”，能工作但分散。** 记录散落在
   `toolchains.lock.json`、`UPSTREAM_PATCHES.md`、`upstream-selection.json`、
   `tools.lock.json`、`fixtures.lock.json`、`third_party/NOTICE.md` 和
   ADR/evidence 中；没有一个 `upstream status` 能看到全貌。
3. **建议增加统一的上游注册表 + drill CLI。** `upstreams.lock.json` 记录每个
   上游的复用方式、pin、契约版本、更新策略、测试和证据；
   `archmeld upstream status/check/diff/drill/record` 负责检查、演练、记录，
   但**永不自动合并/推送**。
4. **CodeGraph 仍是唯一的代码级更新风险点。** 需要把本地 patch 面继续
   缩小、把冲突热点（`main.rs`/`daemon.rs`/`memory.rs`/export）做成
   “变更前预测 + 变更后矩阵”；其余上游按工件/协议契约测试即可。
5. **现有工具已经过期。** `tooling/upstream_delta_drill.py` 仍硬编码
   `590c2ca7` 且把 AT077 标成 partial；实际已合并 `ae4b2e54` 并通过
   AT077。`docs/archmeld/research/upstream-selection.json` 也仍是
   0.21.0/`590c2ca`。这两个应在 U0/U1 中修正，否则重新运行会回退证据。

## 1. 当前集成清单

| 上游 | 复用方式 | 边界 | 代码耦合 | 当前 pin/记录 | 更新方式 |
|---|---|---|---|---|---|
| CodeGraph | 薄分叉（in-tree） | Rust crate/API/CLI | **高** | `ae4b2e54`（0.21.2）；`UPSTREAM_PATCHES.md`、`toolchains.lock.json`、`AGENTS.md`、NOTICE | `git merge --no-ff`；人工解决冲突；回归；更新 pin |
| CodeBoarding | 外部进程 + `analysis.json` 导入 | JSON 工件 | 低 | provider commit/version、`B017` 证据；`import_codeboarding` | 重新生成 analysis.json；importer 契约测试 |
| SCIP | 外部索引器 + protobuf 工件 | `.scip` 工件 | 低 | `scip-protobuf-v0.3`；`scip-clang v0.4.1`、`scip-python 0.6.8` | 重生成 fixture；decoder/importer 测试 |
| LSP 服务器 | 外部进程 + LSP 协议 | stdio JSON-RPC + 能力 | 低 | `tools.lock.json`、profiles、evidence | 版本 probe；能力 profile；真实会话测试 |
| Perfetto/Tracy/VizTracer | 外部采集/转换 + 导入 | trace 工件/可选 trace_processor | 低 | `adapters/traces/fixtures.lock.json` | 重生成 fixture；loss notes；可选后端测试 |
| AOCI-CODE | 只读文本互操作 | `aoci.txt`/`aoci.code.txt` | 无 | v0.2.0-rc4、`cognition.rs` | 格式 fixture；tolerant parser；无二进制 |
| Cytoscape.js | vendored JS | 静态资产 | 无 | 3.30.2 + license | 手工/脚本 vendor；webview 测试 |
| 工具链 | 环境 pin | 编译器/JDK/Go/.NET/Node/Python | 无 | `toolchains.lock.json`、profiles | 版本升级 + doctor/evidence |

## 2. 是兼容层做法吗？

### 2.1 CodeGraph：不是兼容层，是“薄分叉 + 补丁清单”

CodeGraph 的 crate 直接在工作区里编译，ARCHMELD 的 `archmeld-core`
通过 `archmeld-export-v1` NDJSON 消费它。本地补丁在
`UPSTREAM_PATCHES.md` 中逐条记录：workspace 成员、NDJSON 导出、
`--archmeld-export-v1`/`--archmeld-state-root`、状态根隔离、探针测试。
这不是“适配器兼容层”，而是 fork；上游改 API 会直接编译失败或冲突。

优点：性能/类型/调试最好；缺点：上游更新是唯一需要 merge 的路径。

### 2.2 CodeBoarding：数据契约适配，不是代码兼容层

`import_codeboarding` 接收真实 `analysis.json`，验证 `components` 数组、
组件/依赖上限、idempotency、revision，记录 `provider_version`、`run_id`、
命令数量（不执行）和可选模型 provenance。组件/依赖一律是 candidate；
不创建 ledger 实体/关系；引用验证后标 unverified。

这是**工件导入契约**：上游代码不进来，只有 JSON 进来。

### 2.3 SCIP：格式契约适配

`scip.rs` 自己解码官方 protobuf（`third_party/scip/scip.proto`），
`FORMAT = "scip-protobuf-v0.3"`；定义变实体，reference 只生成
`references`，绝不生成 `resolved_call`；工具名/版本记录在 evidence。
这是**二进制工件契约**，不是代码依赖。

### 2.4 LSP：进程/协议适配

`LspSession` 要求显式 spawn grant，记录 argv/cwd/env 名称/pid/请求方法/
shutdown/孤儿进程；profile 只在 `verified` 字面量下授予能力；
`tools.lock.json` 记录各语言服务器版本和哈希。这是**进程边界 +
LSP 协议适配**，不是代码兼容层。

### 2.5 Perfetto/Tracy/VizTracer/OTLP：工件 + 可选进程适配

trace 文件是外部工具产出的不可变工件；导入器负责解码、映射 span 到源码、
记录 loss/coverage；Perfetto trace_processor 是可选 pinned 进程后端。
这是**格式适配 + 可选进程适配**，不是代码依赖。

### 2.6 AOCI-CODE：只读文本互操作

`cognition.rs` 只读 `aoci.txt`/`aoci.code.txt`，解析 Volume/Entry，
映射改动文件；缺失/坏行/CRLF/外来 root 只产生 warning；不执行
AOCI 二进制、不写、不复制代码。这是**文本格式互操作**，不是兼容层。

### 2.7 Cytoscape.js：vendored 资产

固定版本 `3.30.2`，本地 `media/cytoscape.min.js` + license；无 CDN；
webview 测试覆盖。更新是手工 vendor，不是运行时兼容。

### 2.8 工具链：环境 pin

Rust 1.94.1、Node、Python、GCC、JDK、Go、.NET、语言服务器都由
`toolchains.lock.json`/`tools.lock.json`/profiles 记录；`doctor.py`
校验；没有 runtime latest。

**总结：** 当前不是“一个兼容层”，而是六种边界：

```text
代码级：CodeGraph 薄分叉
数据级：CodeBoarding analysis.json、SCIP、trace 工件
协议级：LSP、trace_processor、index helper
文本级：AOCI map
资产级：Cytoscape.js
环境级：工具链 pin
```

只有代码级需要 merge；其余应通过契约测试和版本探测处理。

## 3. 现有更新机制

- **Pin**：`toolchains.lock.json` 的 `codegraph_commit`/`codeboarding_commit`；
  `AGENTS.md`/`UPSTREAM_PATCHES.md`/`third_party/NOTICE.md` 同步。
- **Patch 清单**：`UPSTREAM_PATCHES.md` 记录必要性、路径、是否上游化、AT。
- **真实升级证据**：`docs/release-evidence/UPSTREAM-0.21.2-2026-11-24.json`
  记录 4 个提交、40 文件、冲突、回归、导出。
- **合成 drill**：`tooling/upstream_delta_drill.py` 改名 bridge 字段，
  证明 adapter 测试能发现接口差异。
- **语言/工具锁**：`adapters/languages/tools.lock.json`、profiles、
  evidence；`adapters/traces/fixtures.lock.json`。
- **许可证/来源**：`third_party/NOTICE.md`、`NOTICE`、`THIRD_PARTY_NOTICES`。
- **验收**：AT077 真实上游升级已 passed（2026-11-24）。

## 4. 现有缺口

1. **没有统一注册表。** 上游信息散落在 5-6 个文件；`upstream-selection.json`
   是研究快照，已过期（0.21.0/`590c2ca`）。
2. **drill 工具过期。** `upstream_delta_drill.py` 硬编码 `590c2ca` 和
   `AT077 partial`；重跑会覆盖 `B031-upstream-delta.json`，把已通过的
   AT077 写回 partial。
3. **没有 real-mode drill 命令。** 0.21.2 的合并是手工执行；
   下次升级仍要手工找冲突、跑测试、记录哈希。
4. **没有 artifact 版本门。** CodeBoarding importer 没读
   `analysis_schema_version`；AOCI parser 没读 `#Format-Version`；
   SCIP 有 `FORMAT` 但没在导入时拒绝未知主版本。
5. **没有统一的 `upstream status`。** 无法一眼看到“哪些上游有新版本、
   哪些 pin 落后、哪些契约 fixture 缺失”。
6. **没有 CI 定时检查。** 上游推进不会自动出现在任务列表。
7. **CodeGraph patch 热点集中。** `main.rs` 的参数解析/状态根、
   `daemon.rs` 的 `max_files`、`memory.rs` 的状态根、export 的
   NDJSON 都是未来冲突点；需要继续缩小 patch 面。
8. **构建环境摩擦被当成上游问题。** Windows GNU RocksDB、`-j 2`、
   Python/rust-analyzer PATH、BINDGEN 头文件是环境问题，应独立记录，
   不混入上游 delta。

## 5. 目标架构

### 5.1 统一注册表 `upstreams.lock.json`

```json
{
  "schema_version": "1.0",
  "upstreams": [
    {
      "id": "codegraph",
      "reuse_mode": "thin_fork",
      "source": "https://github.com/codegraph-ai/CodeGraph.git",
      "pin": "ae4b2e54fc8ce1c074523943fc750b20f68bf8aa",
      "observed_version": "0.21.2",
      "license": "Apache-2.0",
      "contract": "archmeld-export-v1",
      "update_policy": "manual_merge_with_drill",
      "last_checked": "2026-11-24",
      "last_drill": "docs/release-evidence/UPSTREAM-0.21.2-2026-11-24.json",
      "tests": ["core_api", "php", "state_root", "export"],
      "owner": "ARCHMELD core"
    }
  ]
}
```

字段：`reuse_mode`（thin_fork / artifact_import / process_adapter /
text_interop / vendored_asset / toolchain）、`pin`、`contract`、
`update_policy`、`last_checked`、`last_drill`、`tests`、`owner`。

### 5.2 契约文件 `contracts/upstreams/<id>.json`

每个上游声明：

- 输入/输出格式与版本字段；
- 支持的版本范围（major/minor/patch 策略）；
- 必填字段、可选字段、未知字段策略；
- 失败策略（fail closed / warn）；
- 版本探测方法；
- 对应测试目录。

### 5.3 `archmeld upstream` CLI

```text
archmeld upstream status [--json]
archmeld upstream check <id> [--remote] [--json]
archmeld upstream diff <id> [--from <pin>] [--to <ref>]
archmeld upstream drill <id> [--ref <ref>] [--keep-worktree]
archmeld upstream record <id> --evidence <file> [--update-lock]
```

- `status`：列出所有上游的 pin、最新、落后提交数、契约版本、上次 drill、
  缺失测试；只读。
- `check`：`git ls-remote`/release API/本地工具 `--version`；不写。
- `diff`：CodeGraph 的 commit range + 变更文件 + patch 热点；artifact
  上游的 schema/fixture 差异。
- `drill`：临时 worktree/branch，执行 merge/import + 测试矩阵，
  生成 evidence；**不 push、不自动改 pin**。
- `record`：人工确认后更新 `upstreams.lock.json`/`toolchains.lock.json`/
  `AGENTS.md`/`NOTICE`，并在同一提交记录证据。

### 5.4 测试目录

```text
tests/upstream/
  codegraph/
    <commit>/          # 可选：真实 checkout 的 delta 摘要
  codeboarding/
    <version>/analysis.json
    <version>/expected.json
  scip/
    <format>/index.scip
  aoci/
    <format-version>/aoci.txt + aoci.code.txt
  traces/
    <tool-version>/capture.*
```

每个目录有契约测试：必须容忍未知字段、拒绝未知主版本、给出明确错误。

### 5.5 CI 定时检查

- 每周一次 `archmeld upstream status/check`，输出报告/issue；
- 不自动 merge、不自动提 PR、不自动改 pin；
- 安全/关键修复可以手动触发 drill。

## 6. 按类型的更新 playbook

### 6.1 CodeGraph（唯一代码级更新）

**目标：** 用一个可重复的 merge + 测试 + 证据流程，替换手工升级。

1. **Preflight**
   - `archmeld upstream check codegraph`：`git ls-remote`，确认新 commit/tag；
   - `archmeld upstream diff codegraph --from <pin>`：commit list、diffstat、
     变更文件、release notes；
   - 标记本地 patch 热点：`main.rs`、`daemon.rs`、`memory.rs`、
     `export/ndjson.rs`、`export/mod.rs`、`Cargo.toml`、`Cargo.lock`。
2. **Drill（临时分支）**
   - `git worktree add <temp> <pin>` 或直接在 `archmeld/initial` 上
     `git merge --no-ff <ref>`；不 rebase 已发布分叉；
   - 解决冲突：保留上游结构，适配本地 patch；不删上游测试；
   - 编译：`cargo check ... -j 2`（Windows GNU 需 `-j 2`、Strawberry
     PATH、`BINDGEN_EXTRA_CLANG_ARGS`、TMP 在 F:）；
   - 测试矩阵：core/API 全量；语言 parser 变更时 PHP/七语言；状态根；
     export/helper；MCP 握手；AT077 端到端导出。
   - 记录：命令、cwd、exit、日志哈希、冲突文件、解决说明、产物哈希。
3. **判定**
   - 全绿 → `upstream record` 更新 pin + NOTICE + `UPSTREAM_PATCHES.md`；
   - 有失败 → 保留旧 pin，记录失败原因和最小复现，不部分合并。
4. **Patch 最小化**
   - 本地 patch 尽量放进独立模块/flag；避免改上游热文件；
   - 能上游化的（NDJSON 导出、state root、max-files）提交 upstream，
     减少分叉面；
   - 不在上游文件里做无关格式化/重命名。
5. **回滚**
   - 升级前 tag；保留上一个 pin 和 release 二进制；
   - `upstream rollback` 文档化：checkout 上一个 pin、重建、验证。
6. **禁止**
   - 自动跟随 latest；静默换版本；用 `git reset --hard`/`git clean -fd`
     作为恢复；合并 public snapshot 分支（它是脱敏快照，不是历史）。

### 6.2 CodeBoarding（工件导入）

1. 在隔离环境用 pinned CodeBoarding 生成 `analysis.json`；
2. 记录 provider version/commit、run id、命令数量、模型 provenance；
3. 跑 importer 契约测试：components/relations/limits、缺失引用、
   未知字段、candidate-only；
4. 若上游 schema 变化：新增 fixture 版本，不覆盖旧 fixture；
5. 失败关闭：缺 `components` 数组直接拒绝；未知字段忽略但记录 warning。

### 6.3 SCIP

1. 用 pinned indexer（scip-clang/scip-python）生成 `.scip`；
2. 记录 indexer 版本、文件哈希、快照绑定；
3. decoder 只读 `scip-protobuf-v0.3`；未知字段容忍；未知主版本 warning；
4. 测试：documents 必须在快照；reference 不生成 resolved_call；
   UTF-8/16/32 范围；重复定义诊断；
5. 更新：新增 fixture 版本，跑真实索引测试；不接 runtime。

### 6.4 LSP 服务器

1. `tools.lock.json` 固定版本/路径/哈希；
2. 启动前 `--version`/initialize `serverInfo` 探测；
3. 能力 profile 只在真实测试通过后写 `verified`；
4. 更新：新版本 → 跑语言 profile（definition/reference/rename 预览/
   进程审计）；失败则 capability 降级为 unverified，不假装支持；
5. 不改 LSP 协议本身；服务器差异放 `initialization_options`。

### 6.5 Trace 工具

1. `fixtures.lock.json` 固定工具版本/归档哈希/编译参数；
2. 重新采集 fixture，保留 raw 工件；
3. 导入器测试：span→source、loss notes、多 clock 域、无伪造 call；
4. trace_processor 可选后端按 pinned 版本单独测试；失败回退 in-process；
5. 不把 span-parent containment 写成 call。

### 6.6 AOCI-CODE

1. 只读解析 `aoci.txt`/`aoci.code.txt`；不执行二进制、不写、不复制；
2. 读取 `#Format-Version`/`#ARCHMELD-MAP`；未知主版本只 warning；
3. 每个格式版本放 fixture：neutral root、CRLF、坏行、外来绝对 root；
4. 上游 rc 变化只影响 parser/fixture，不影响核心证据模型；
5. FSL 边界：format 思想可参考，代码不复制。

### 6.7 Cytoscape.js / vendored 资产

1. 固定版本 + license + sha256；
2. 更新走 vendor 脚本/手工替换 + webview 测试（local-only，无 CDN）；
3. 不运行时下载；不在 webview 执行仓库内容。

### 6.8 工具链

1. `toolchains.lock.json`/`tools.lock.json` 记录版本/路径/哈希；
2. `tooling/doctor.py` 校验；升级前跑全量；
3. 工具链变化与上游 delta 分开记录，避免把环境问题写成上游缺陷；
4. 不 runtime latest。

## 7. 兼容性策略

- **Exact pin**：所有上游按 commit/version；禁止 latest。
- **Major 失败关闭**：未知主版本拒绝导入/启动，给出升级指引。
- **Minor 警告**：未知 minor 字段忽略但记录 warning；fixture 覆盖。
- **N-1 支持**：artifact/format 至少保留上一版本 fixture；CodeGraph
  只支持当前 pin，升级窗口明确。
- **未知字段**：保留/忽略并记录；不静默丢弃会影响语义的字段。
- **版本探测**：CodeGraph commit、CodeBoarding provider version、
  SCIP tool version、LSP serverInfo、AOCI format version、trace tool version。
- **证据绑定**：每次 drill 产出 `UPSTREAM-<id>-<version>.json`，包含
  pin、delta、冲突、命令、测试、哈希、限制。

## 8. 自动化设计

### 8.1 `upstream status` 输出

```text
id            mode              pin        latest      behind  contract              last drill
codegraph     thin_fork         ae4b2e54   ae4b2e54    0       archmeld-export-v1    UPSTREAM-0.21.2
codeboarding  artifact_import   a38a8d15   <remote>    ?       analysis.json         B017
scip          artifact_import   v0.3       v0.3        0       scip-protobuf-v0.3    B009
lsp           process_adapter   per-tool   per-tool    ?       lsp-3.17              B010-B014
aoci          text_interop      d28c8566   d28c8566    0       cognition-volumes/v1  M2.5-C
```

只读；`--json` 供 CI。

### 8.2 `upstream drill codegraph` 流程

```text
1. verify clean worktree
2. fetch upstream
3. create temp worktree/branch at current pin
4. merge target ref --no-ff
5. resolve conflicts (human; CLI reports files)
6. cargo check -j2 with pinned env
7. targeted tests by changed paths
8. core/API full regression
9. export/helper end-to-end
10. write evidence JSON + logs
11. leave worktree for review or remove
12. never push; never update lock automatically
```

### 8.3 CI

- 每周 `upstream check`；报告到 issue/artifact；
- 不在 CI 自动 merge；
- 安全修复可手动触发 `drill`；
- `doctor.py` 继续校验工具链。

## 9. 实施计划

| 阶段 | 内容 | 估算 |
|---|---|---|
| **U0** | `upstreams.lock.json` + `upstream status`（只读）；修正过期 pin/AT077 记录 | 1 天 |
| **U1** | `upstream diff/drill codegraph` real-mode；替换合成 drill 的硬编码 | 2 天 |
| **U2** | CodeBoarding/SCIP/trace/AOCI 契约 fixture + 版本门 | 1-2 天 |
| **U3** | LSP/trace 版本 probe + capability 降级测试 | 1 天 |
| **U4** | 每周 CI `upstream check` + 证据模板 | 0.5 天 |
| **U5** | 文档/playbook/回滚演练 | 0.5 天 |

## 10. 验收

- `archmeld upstream status --json` 能列出全部上游，pin/latest/contract/
  last drill 无缺失；过期记录被修正。
- `upstream drill codegraph --ref <new>` 在临时分支完成 merge + 测试 +
  evidence；不 push、不改 lock。
- `upstream record` 只在人工确认后更新 pin/NOTICE/UPSTREAM_PATCHES。
- 每个 artifact 上游有至少一个版本 fixture 和未知字段测试。
- 重新运行 AT077 演练能产出 passed 证据，而不是 partial。
- 不出现自动 latest、自动 merge、自动改 pin。

## 11. 风险

- **上游 churn**：CodeGraph 接口变更频繁；靠 patch 最小化 + 每次 drill。
- **契约漂移**：artifact 格式可能变；靠版本门 + fixture。
- **许可证变化**：每次更新检查 license；不静默接受。
- **环境问题混淆**：Windows GNU/RocksDB、PATH、BINDGEN 是环境；单独记录。
- **public/private**：public snapshot 是脱敏快照，不是历史；不要 merge。
- **工具自身过期**：drill 脚本/registry 必须跟随 pin；否则会写回旧证据。

## 12. 结论

当前集成不是“统一兼容层”，而是**按耦合度分层**：只有 CodeGraph 需要
代码级 merge，其余都是工件/协议/文本/资产/环境边界。更新策略应该顺着
这个分层走：

- CodeGraph：pin + 最小 patch + real drill + 测试矩阵 + 证据 + 回滚；
- CodeBoarding/SCIP/trace：版本化 artifact contract + fixture + 失败关闭；
- LSP/trace_processor：进程适配 + 版本 probe + capability 降级；
- AOCI：文本格式互操作 + tolerant parser + 无二进制；
- Cytoscape/工具链：版本 + 哈希 + license + doctor。

下一步先做 **U0 + U1**：统一注册表和 `upstream status`，以及把
`upstream_delta_drill.py` 从合成/过期状态改成 real-mode；这两步能立即
消除“上游更新靠手工记忆”和“重跑会回退 AT077 证据”的问题。
