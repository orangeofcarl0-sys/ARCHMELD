# E 方案详细分析：证据绑定认知图

分析日期：2026-11-24。前置：M2.5-D 已通过成功线（完整仪式 B 3/3 vs A 0/3，
tokens 0.942x，6/6 正确）。本文只做设计与计划，不实现代码。

**实施状态（2026-11-25）**：E1/E2 已实现（`archmeld.map.txt` 格式 +
`map check|status|maintain` + `brief(recipe=map)` + status map +
`inspect(kind=map_entry)`；10 个 core map 测试、5 个 brief 测试、1 个 MCP
测试、1 个 CLI 测试；full core/API 238/0/0）。E2.1 补齐语义边界标签：
`semantic_claims=unverified`、`verification=not_run`、
`validation=not_run|refs_checked`，`brief(recipe=map)` 增加独立
`map_state`。E2.2 让 `affected()` 返回 Entry 的 `api`/`constraint`/`refs`
与顶层 `state`/`snapshot_match`/`map_digest`/`binding`；所有读面输出
`map_digest`，`check_map`/`summarize`/`maintain` 输出 `freshness`；修复
`entry_report` 无地图、`maintain` 无快照和 maintain markdown。full
core/API 240/0/0。**E4a 已执行并 no-go（2026-11-25）**：pi 1.0.4 +
`omni-anthropic/deepseek-v4.1-flash`，26 文件 nonlexical fixture；A 无地图
2/3 正确，B 有地图 1/1 正确，门禁要求 A ≤1/3 未通过。E 停在 E2 只读，
E3（`map apply`）/E4b/规则 v3 不推广；详见
`docs/release-evidence/E4A-NOGO-2026-11-25.json`。再评估的顺序
**E4a → E3 → E4b** 保留为设计记录，但 E4a 已否决后续阶段。详见第 12 节与
`docs/assessments/AOCI-CORE-AND-ARCHMELD-COGNITION-GAP-2026-11-25.md`
第 12 节。证据
`docs/release-evidence/{E1-MAP-CHECK,E2-MAP-MAINTAIN-BRIEF}-2026-11-25.json`。

## 0. 结论

1. **E 值得做，但先做只读版，不做大而全的“第二套认知系统”。** 先交付
   `map check/status`（校验 + 漂移），再决定是否做 `map apply` 和规则升级。
2. **E 补的是 ARCHMELD 缺失的“语义认知层”，不是反 AOCI。** AOCI 的核心
   思想是“模型作者、受治理、可持久化的系统认知”，ARCHMELD 有证据但没有
   这层；E 是它的证据绑定版。详细分析见
   `docs/assessments/AOCI-CORE-AND-ARCHMELD-COGNITION-GAP-2026-11-25.md`。
3. **E 的差异化不是“持久文本”，而是“每条关系都能回到快照证据”。**
   AOCI 的 Entry 是模型散文；E 的 `R` 必须是可解析的 file/entity/relation
   引用，候选边不得写成 confirmed，缺失/歧义/陈旧必须显式。
4. **E 不放进 MCP 写面。** MCP 仍是 8 个只读工具；写入走 CLI
   `map apply`（候选文件 + 内容哈希 + 快照绑定）。地图文件是普通仓库文本，
   模型可以用宿主文件工具编辑，但必须经 `map check`/`map apply` 才能成为
   正式地图。
5. **E 的第一优先级是“防止错误认知被固化”。** 机器只能验证引用和解析状态，
   不能证明 F/A/S 的语义正确；因此输出必须带 `semantic_claims=unverified`，
   并明确它不是测试通过、不是已解析调用图、不是单一 readiness 分数。
6. **E 的前置缺陷：Python 跨模块同名绑定（已修复）。** `from pricing
   import apply_discount` 曾在 `codegraph-server` 里绑到
   `legacy.apply_discount`；2026-11-25 的修复为每个 Python 文件建立导入
   绑定，并让无绑定的重复自由函数保持 unresolved。E1 仍只允许
   `file:`/`entity:` 引用；`relation:` 语法与解析尚未实现，是独立的
   E3/E5 任务，不再被该缺陷阻塞。证据
   `docs/release-evidence/PYTHON-IMPORT-BINDING-2026-11-25.json`。
7. **E4a 已执行并 no-go。** 结构校验不等于语义价值；clean nonlexical
   fixture 上 A 无地图 2/3 正确、B 有地图 1/1 正确，门禁 A ≤1/3 未通过。
   E 停在 E2 只读；不建 `map apply`（E3），不做 E4b，不安装规则 v3。
   详见 `docs/release-evidence/E4A-NOGO-2026-11-25.json` 与
   `docs/evidence/e4a-2026-11-25/RESULT.md`。

## 1. 目标与非目标

### 目标

- 让模型在**新会话**里用一份可读、可校验的文本快速恢复系统级认知，
  而不是每次重新 grep/读文件。
- 每个 `R`/`A` 引用都能在指定快照中解析；解析不了就是 `orphan`/`unknown`，
  不是“看起来像”。
- 代码变化后，`map maintain` 能列出哪些 Entry 需要重新读证据，而不是
  静默保留旧语义。
- 与 AOCI 共存：有 AOCI 时 brief 显示 AOCI；有 ARCHMELD map 时显示
  ARCHMELD map；两者都不冒充调用图。
- 低认知成本：一份文件、固定字段、固定命令、预算可见。

### 非目标

- 不是知识图谱/向量库/第二套图数据库。
- 不是自动生成语义：不允许从 AST、文件名、正则、模板“生成 F/R/A/S”。
- 不是 `brief`/`review`/`source` 的替代；行为证据仍要回源码。
- 不是单一 readiness 分数；不隐藏 candidate/unknown/stale/partial。
- 不是多 Agent 写权限系统；M4 之前只有一个显式 CLI writer。
- 不是提交门禁；M3 只做非阻塞 `commit-review` 信号。

## 2. 与 AOCI-CODE 的关系

| 维度 | AOCI-CODE | E |
|---|---|---|
| 产物 | `aoci.txt` + volumes | `archmeld.map.txt` 单文件（可配置） |
| 语义来源 | 模型作者 F/R/A/S | 模型作者 F/A/S，R 是可解析引用 |
| R 的性质 | 路径级强关系，程序不验证 | file/entity/relation 引用，程序逐条解析 |
| 候选/已解析 | 无调用图 | 保留 `candidate`/`confirmed`/`unknown` |
| 写入 | 3 个 MCP 写工具 + CAS 治理 | CLI `map apply`；MCP 只读 |
| 规模 | 约 14 万 tokens/会话（ERP 案例） | 默认预算 40k tokens，按 scope 裁剪 |
| 用途 | 系统认知 | 系统认知 + 影响面 + 漂移 |
| 许可证 | FSL-1.1-MIT | 不复制其代码；格式思想引用并注明 |

E 不复制 AOCI 的实现或私有协议。FRAS 的字段思想是公开的；E 使用自己的
schema、自己的校验器和自己的引用语法。

## 3. 用户场景与 working loop

| 场景 | 用户说法 | 首步 | 循环 | 输出 | 边缘 |
|---|---|---|---|---|---|
| 接手项目 | “这个项目怎么组织/入口在哪” | `brief(recipe=project)` | `map status` → 读 map → `source`/`context` | 一屏 brief + map 状态 | 空仓库/超大仓库/vendor/generated |
| 模块整理 | “这个模块职责是什么/重复在哪” | `map status` | 读 entry → `query impact` → `source` | entry + 受影响 caller | 无 entry/entry 陈旧 |
| 改动评估 | “这次改动安全吗” | `brief(recipe=change)` | change brief 列 affected entries → `review` → `source` | affected map entries + counts | 改动文件无 entry；entry 引用已删除符号 |
| 不良实现排查 | “这里有没有隐藏耦合” | `brief(recipe=change)` | `query` → `context` → map 的 R/S | 证据 + 约束 + unknown | candidate 边被误读为 confirmed |
| Runtime | “实际怎么跑/为什么慢” | `trace-import` | runtime inspect → map 的 runtime 注记 | runtime card + map 关联 | span-parent 不是 call；map 不冒充 trace |
| 多 Agent 交接 | “把结论交给下一个 agent” | `brief(recipe=map)` | 读 map + unknown + next actions | handoff packet | 并发写、stale、单 writer |
| Commit 监督 | “每次 commit 帮我看一下” | M3 `commit-review` | commit card 内 `map maintain` 摘要 | 非阻塞提示 | merge/empty/rename/shallow |

**抽象阶梯**：file entry → module entry → architecture（后续）→ runtime
（后续）→ change（brief 已有）→ commit（M3）。E1 只做 file entry，避免
一上来就做符号级/组件级语义。

## 4. 产物格式

### 4.1 路径与提交

默认：`<workspace>/archmeld.map.txt`，**提交进 Git**。理由：

- `.archmeld/` 当前被 snapshot/drift 排除（会话卡不制造假 drift），把它
  放进去会让地图变化不可见；地图应该像代码一样被 review。
- 根文件对模型可见，不需要额外工具就能读；AOCI 的经验是“索引在 Git 里”
  才有跨会话价值。
- 可通过 `--map <path>` 覆盖；一个 workspace 只允许一个正式地图。

### 4.2 头部

```text
#ARCHMELD-MAP: 1
#snapshot: sha256:...
#commit: abc123
#scope: code
#author: host-model
#updated_at_unix: 1795530000
#budget_tokens: 40000
#observe: vendor/**, generated/**
#exclude: dist/**, target/**
```

- `snapshot` 是作者读证据时的快照；没有它地图就是 `unbaselined`。
- `commit` 只作参考；以 snapshot 为准（工作树可能未提交）。
- `observe`/`exclude` 是 scope 声明：observe 文件不要求 Entry，但漂移仍可见；
  exclude 完全不管理。

### 4.3 Section 与 Entry

```text
===src/===
pricing.py[L2D4C7M]: F:Computes order pricing and the discount applied at checkout. | R:entity:function:src/pricing.py::apply_discount,file:src/checkout.py | A:apply_discount(value)->float | S:Callers assume a 0.9 multiplier in analytics and invoice constants; changing it changes those expectations.
checkout.py[L2D4C7M]: F:Converts a cart into a checkout total. | R:entity:function:src/pricing.py::apply_discount | A:checkout(value)->float | S:-
```

字段：

- `F`：一个职责命题，≤240 字符；不是清单。
- `R`：逗号分隔的引用；≥1 项或 `-`。引用语法见 5 节。
- E1 的对象不限于代码：配置、文档、运维脚本也可以是 `file:` 对象；
  `entity:` 只在有图时使用。这是 AOCI 语言无关性的证据绑定版，不要求
  每个对象都有解析器。
- `A`：稳定的外部契约；≤240 字符；`-` 表示没有有意义的稳定契约。
- `S`：高熵、非显然、删掉会导致改错的约束；≤400 字符；`-` 允许。
- Tag：`L` 层、`D` 域、`C` 关键度 1-9、`M` 规模 XS/S/M/L/XL。
  E1 允许无 Tag；Tag 只用于导航/预算，不参与证据判定。
- 文件名是 section 内的 basename；禁止绝对路径；路径分隔统一 `/`。

### 4.4 预算

- 单 Entry 目标 80-150 tokens；默认整图预算 40k tokens（约 250-350
  个文件级 Entry）。
- `map status` 报告 `chars`、`tokens_estimate`、`largest_entries`、
  `over_budget`；超预算只警告，不截断。
- `brief` 只返回 affected/summary，不返回整图；整图由模型按需读文件或
  `map status --entries` 分页。

## 5. 证据绑定模型

### 5.1 引用语法（E1）

```text
file:<repo-relative-path>
entity:<kind>:<repo-relative-path>::<qualified-name>
```

- `kind`：`file`、`module`、`symbol`、`type`、`component`、`process`。
- `entity` 的 `qualified-name` 使用快照里的 `qualified_name`（当前是
  `path::name`，PHP 方法可能是 `Class::method`）；解析时按 path+kind+name
  查找，绝不按裸名猜。
- 允许后缀 `@candidate`、`@unknown`：
  - `@candidate`：引用存在，但只由 `static_call_candidate`/`references`
    支撑；`map check` 要求它不能同时被写成 `confirmed`。
  - `@unknown`：明确表示未绑定；必须带理由，如
    `entity:function:src/x.py::foo@unknown(dynamic dispatch)`。
- `relation:` 引用推迟到 E2；在 Python 同名误绑修复前不开放。

### 5.2 解析与状态

`map check` 对每个引用输出：

| 状态 | 条件 | 是否错误 |
|---|---|---|
| `confirmed` | 文件存在；entity 存在且唯一；relation 的 resolution 为 declared/resolved/observed | 否 |
| `candidate` | 只有 candidate/references 关系支撑，或引用带 `@candidate` | 否，计数 |
| `unknown` | 引用带 `@unknown(reason)` | 否，计数 |
| `orphan` | 文件/实体不存在 | 是 |
| `ambiguous` | 同一 path+kind+name 多个实体 | 是 |
| `stale` | 地图 header snapshot 不是最新；或文件 blob 已变 | 警告（可 `--fail-on stale`） |

**不嵌入 `syn.*`/`rel.*` 作为主键。** 语法身份包含文件 blob 和行号，行移动
就会变；地图用逻辑引用，`map check` 在当前快照解析。可选 `#evidence:` 行
记录作者当时看到的 relation id，仅作审计，不作为解析依据。

### 5.3 语义边界

- `F`、`A`、`S` 是模型语义，机器只能做格式/引用校验；`map check` 输出
  `semantic_claims: "unverified"`。
- `R` 的 `confirmed` 只表示“引用在快照中存在且解析状态允许”，不表示
  “运行时真的调用”。
- 候选边永远不能因为地图写了 `confirmed` 就升级；`map check` 直接报错。
- `S` 里不得写“测试通过”“已解析调用”“runtime 已验证”，除非有对应
  evidence id；E1 先做文本扫描警告，E2 再做结构化 `evidence:` 引用。

## 6. 作者循环

1. `archmeld map status --state --project --workspace`：看
   entries/missing/orphan/stale/budget。
2. 读证据：`archmeld_source`/`query`/`context`（MCP）或 CLI 等价命令。
3. 模型写 `archmeld.map.candidate.txt`（或直接编辑正式地图，但不建议）。
4. `archmeld map check --candidate <file>`：返回逐条错误/警告。
5. 修正后 `archmeld map apply --candidate <file> --workspace ...`：
   - 重新校验；
   - 要求 candidate 的 header snapshot 等于当前最新快照（`--allow-stale`
     可显式放宽）；
   - 拒绝未解析引用、拒绝 confirmed 候选边、拒绝重复 Entry；
   - 原子写入正式地图（临时文件 + rename）；
   - 记录 receipt：`{map_digest, previous_digest, snapshot_id, entries,
     actor, applied_at}`，写入 state 的 `map_receipts`，不进 Git。
6. 用户提交地图与代码；`map maintain` 在下次改动后列出 stale。

**禁止**：程序生成/预填/压缩/改写 F/A/S；模型可以先读机器收集的候选文件
列表，但语义必须自己写。

## 7. 新鲜度与漂移

- `map_stale`：header snapshot ≠ 最新快照或当前 workspace 快照。
- `entry_stale`：Entry 对应文件的 blob digest 与 header snapshot 不同。
- `orphan`：Entry 路径不在快照；`missing`：快照文件没有 Entry 且不在
  observe/exclude。
- `ambiguous`：逻辑引用在当前快照有多个候选。
- `unbaselined`：没有 header 或地图从未 apply。
- `map_dirty`：工作树中的地图与快照/提交不同（review 可见）。

`map maintain` 用 `review_workspace_diff`/快照 blob 比较，列出：
- 哪些 Entry 需要重新读证据；
- 哪些文件新增/删除；
- 哪些引用变成 orphan/ambiguous；
- 不自动改写任何 Entry。

`brief(recipe=change)` 增加 `cognition` 块（provider=`archmeld-map`）：
affected entries、stale count、missing count、warnings；不把整图塞进 brief。

## 8. 读取面

- `brief`：E2 增加 `recipe=map`（或 `recipe=project` 内嵌 map 状态）。
  建议新增 `recipe=map`，保持 `project/change/auto` 语义不变。
- `inspect(kind=map_entry, id=<path>)`：返回 Entry、解析后的 refs、
  evidence ids、stale/orphan 状态；不新增 MCP 工具，仍是 8 个。
- `status`：增加 `map: {present, entries, stale, missing, budget}` 的摘要。
- Markdown 卡片：在现有 Cognition map 段显示 provider、entries、
  affected、warnings；AOCI 与 ARCHMELD map 可同时出现，分别标注。
- 不新增 `archmeld_map` 工具；若 E4 证明需要，再按 S11 上限评估第 9 个只读工具。

## 9. 写入面与权限

- E1-E3：只有 CLI `map apply` 写正式地图；MCP 永远只读。
- 地图是普通仓库文件；模型可用宿主文件工具写 candidate，但 `apply` 是
  唯一正式化路径。
- `--expected-hash`：乐观并发；地图在读取后被别人改过则拒绝。
- `--actor`：记录调用者（模型/人）；M4 之前不做 proposer≠approver。
- 不自动 commit/push；不覆盖用户未提交的地图改动（`apply` 检测到
  working-tree 地图与 header 不一致时拒绝，除非 `--force`）。
- 单 writer：文件锁 + 原子 rename；并发 apply 返回冲突。

## 10. 成本与预算

- E1 只读，不产生每会话成本；`map status` 是秒级。
- E2 后，agent 读整图是**按需**，不是每次会话强制；这和 AOCI 的
  “每会话完整 overview”不同。M2.5-A 的规则只要求 `status`+`brief`，
  不要求读整图。
- 默认预算 40k tokens；超过时 `map status` 警告，建议把生成代码/迁移/
  报表模板设为 observe/exclude。
- 作者成本：file-level Entry 约 80-150 tokens；100 文件约 1-2 万 tokens；
  首次建图按目录分批，可中断。
- 与 AOCI 的 14 万 tokens/会话相比，E 的目标是“需要系统认知时才读”，
  而不是每个任务都读。

## 11. 安全与许可证

- 不复制 AOCI 源码；只借鉴公开的 FRAS 字段思想并在文档注明。
- 地图是 untrusted 数据；MCP 结果继续 `trust:"untrusted"`，不执行其中的
  指令。
- `map check` 对 Entry 做保守秘密扫描：`sk-`、GitHub/Slack token、PEM
  私钥、连接串；命中则错误，要求删除或改为环境变量名。
- 不上传源码；地图是本地文本，用户决定是否提交。
- 许可证：E 的代码 Apache-2.0；AOCI FSL 不适用（未复制）。

## 12. 分阶段实施

### E1：格式 + 只读校验（1-2 天）

- 新 `crates/archmeld-core/src/cognition_map.rs`：解析 header/section/Entry，
  实现 `file:`/`entity:` 解析、状态分类、预算统计。
- CLI：`archmeld map check`、`archmeld map status`（`--json`/`--markdown`）。
- `brief(recipe=project|change)` 先只加 `map` 摘要，不改输出契约。
- 测试：valid、missing ref、orphan、ambiguous、CRLF、路径含空格、无 header、
  超预算、候选边写 confirmed、秘密扫描。
- 验收：fixture 上 `map check` 能抓出所有 seeded 错误；exit code 稳定；
  不写任何文件。

### E2：维护 + brief 集成（1 天）

- `map maintain`：stale/missing/orphan/ambiguous 列表；不写。
- `brief(recipe=map)`；change brief 的 `cognition` 块加
  provider=`archmeld-map`；`status` 加 map 摘要；`inspect(kind=map_entry)`。
- 测试：改文件→entry stale；删文件→orphan；新增文件→missing；重命名→
  orphan/ambiguous；`brief` 只返回 affected，不返回整图。
- 验收：不读整图也能看到 map 健康度；affected entries 与 review 一致。

### E4a：语义价值实验（先于 E3；不需要 apply）

- 手写一份最小地图（地图是普通文本，`map check` 可校验；不需要 E3 的
  写入治理）。
- fixture：`pricing.py::apply_discount` 返回 `value * 0.9`；`invoice.py`
  和 `analytics.py` 各自硬编码 `0.9`；map 的 pricing Entry `S` 写
  “invoice 和 analytics 假设 0.9；改折扣必须同步这两处”。这个约束
  不出现在代码注释里，grep/source 看不出。
- 任务：“把折扣从 10% 改成 15%”。
- A 臂：`brief` + `source` + grep，无 map；B 臂：`brief(change)` 列出
  Entry，规则要求编辑前读 Entry。复用 M2.5-D harness，每臂 ≥3 次。
- 记录：是否同步修改 invoice/analytics、是否引用 `S`、tokens、是否把
  candidate 说成 confirmed、是否出现无依据结论。
- Go：B 在 ≥2/3 次运行中避免“只改 pricing”的错误，或明确引用 `S` 并
  同步两处；tokens ≤ A 臂 1.3 倍；0 candidate 冒充。
- No-go：A/B 结果无差异，或 B 只是多读文本但决策不变 → 停在 E2 只读，
  不建 E3，不推广规则 v3。

### E3：最小 apply（E4a go 后，1-2 天）

- `map apply --candidate --expected-hash --actor`；校验 header snapshot、
  refs、confirmed 候选边、secret、并发；原子写 + receipt
  （`map_digest`/`previous_digest`/snapshot/actor/time）。
- 不做 journal/恢复状态机，除非真实并发证明需要。
- `import-aoci` 不进 E3 必做项；若做，只作为 `provider=aoci` 的只读外部
  视图，或标 `origin=aoci`/`reauthor_required=true`，不得直接进入正式
  map。
- 规则 v3（仅 E4a 通过后）：编辑前 `status`/`brief`；大改后
  `map maintain`；改动文件有 Entry 时最终答复引用 Entry 或明确说“未
  参考”；不自动生成；map 只用于系统认知，行为仍回 source。
- 测试：未解析引用拒绝；confirmed 候选边拒绝；并发 apply 冲突；地图被
  外部改动拒绝；成功 apply 后 check 通过；secret 命中拒绝。
- 验收：apply 后地图可提交；re-run check 幂等；receipt 可审计。

### E4b：规模与维护（E3 后，1 天）

- 在 ARCHMELD 自身或一个 50-100 文件仓库建 file-level map；记录：
  entries、tokens、作者耗时、check 错误数、stale 检测、binding 分布
  （`file` vs `entity`）、agent 是否在编辑前读 map（复用 M2.5-D
  harness，3 次）。
- 连续 3 次会话观察维护是否发生；不维护就降级为只读提示。
- 抽样 `F/A/S` 分类：复述图事实 / 图外洞察 / 错误；要求有一定比例的
  图外洞察，错误可解释。
- Go 条件：map ≤40k tokens；check 0 未解释错误；3 次中 ≥2 次编辑前读
  map；agent 不把 candidate 写成 confirmed；无源码上传；维护发生。
- No-go：地图超预算/无人维护/agent 忽略；停在 E2 只读，不推广 E3。

### E5（可选，另立 ADR）

- M3 `commit-review` 内嵌 `map check`（非阻塞）。
- HTML/VS Code map 视图。
- M4 scoped map writer / proposer≠approver。
- 符号级/组件级 Entry（只在 file-level 证明有效后）。

## 13. 验收与实验

### 单元/集成

- `map check` 对 10 类 seeded 错误全部报出，exit code 正确。
- `map maintain` 在文件修改/删除/新增/重命名后给出正确分类。
- `brief(recipe=change)` 的 affected entries 与 `review` 的 changed files
  一致；无 map 时不报错。
- `map apply` 拒绝未解析引用、confirmed 候选边、并发冲突、秘密命中。
- `map check` 对 AOCI `aoci.txt` 不误读；两个 provider 同时存在时分别标注。

### 试点指标

| 指标 | 目标 |
|---|---|
| 地图 tokens | ≤40k（50-100 文件） |
| 首次作者耗时 | 记录，不设硬门槛 |
| check 未解释错误 | 0 |
| 编辑前读 map 的 agent 比例 | ≥2/3 |
| candidate 写成 confirmed | 0 |
| 源码上传/网络 | 0 |

## 14. 风险与备选

- **语义正确性无法机器证明。** 地图可能固化错误理解；必须显示
  `semantic_claims=unverified`，并保留 source 证据入口。
- **地图陈旧比没有地图更危险。** header snapshot + maintain 必须可靠；
  如果 3 次改动后无人更新，E 应降级为只读提示，不进入规则强制。
- **图关系误绑。** Python 同名 bug 会让 `relation:` 引用指向错误目标；
  E1 先只做 file/entity，修复后再开放 relation。
- **成本。** 40k 预算是硬约束；超预算用 observe/exclude，不静默截断。
- **AOCI 重复。** 两个 provider 可共存；不合并、不互相升级证据。
- **写路径风险。** `map apply` 必须 CAS/原子/哈希；绝不由 MCP 写。
- **备选 A：** 只保留 AOCI 互操作，不做 E。适合“地图无人维护”的情况。
- **备选 B：** 机器生成 file 列表 + 模型只写 S。降低作者成本，但仍需模型
  语义，且容易变成模板；只在 E4 证明作者成本过高时考虑。
- **备选 C：** 符号级地图。成本高、漂移快；E1 不做。

## 15. 开放决策（建议）

| 决策 | 建议 |
|---|---|
| 地图路径 | `archmeld.map.txt` 根文件，提交 Git；`--map` 可覆盖 |
| 粒度 | E1 file-level；symbol 级留到 E5 |
| Tag | 可选 `[LxDxCxM]`；无 Tag 合法 |
| MCP 面 | 不加工具；`brief(recipe=map)` + `inspect(kind=map_entry)` |
| 写入 | 候选文件 + `map apply`；MCP 永不写 |
| 预算 | 默认 40k tokens；超预算警告，不截断 |
| relation 引用 | 等 Python 同名绑定修复后再开放；E1 只 file/entity |
| 自动生成 | 禁止生成 F/A/S；只允许机器列文件/候选引用 |
| 价值验证 | E4a 先于 E3：用“只有地图知道”的约束做 A/B 结果实验；无差异则停在 E2 |
| AOCI 迁移 | 默认保持只读外部视图（`provider=aoci`）；`map import-aoci` 非必做，若做须标 `origin=aoci`/`reauthor_required=true`，不复制实现 |
| 语义标签 | 所有 map 读面输出 `semantic_claims=unverified`/`verification=not_run`；未解析 refs 的摘要标 `validation=not_run` |
| 绑定强度 | 报告 `file`/`entity` 引用计数；`file:` 只证明字节，不证明 F/A/S |
| Git | 地图与代码一起提交；apply 不自动 commit |
| 提交监督 | M3 只做非阻塞 `map check` 摘要 |

## 16. 结论

E 的价值是“补上 ARCHMELD 缺失的语义认知层”：把系统理解变成可校验、可
漂移、可交接的仓库资产。AOCI 提供了这套认知层最完整的设计参考，但 E 不
复制它的实现，而是用 ARCHMELD 的证据绑定、候选/已解析边界和 CLI-only
写面重做。D 已经证明会话契约能改变 agent 行为，E 应建立在同一机制上：
先只读校验和维护，再考虑作者写入；先 file-level，再 relation/架构；
先 40k 预算，再谈大规模。

建议下一步只做 **E1**：格式、`map check`、`map status` 和测试。E2/E3 在
E1 的真实仓库数据出来后再决定；E4 通过前不推广。AOCI 的完整思想与功能
拆解见 `docs/assessments/AOCI-CORE-AND-ARCHMELD-COGNITION-GAP-2026-11-25.md`。
