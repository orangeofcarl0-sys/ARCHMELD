# AOCI 的核心思想与 ARCHMELD 的认知能力缺口

分析日期：2026-11-25。对象：`aoci-spec/aoci-code` v0.2.0-rc4 的核心设计，
以及它对 ARCHMELD 的角色。本文不讨论“谁替代谁”，只回答两个问题：

1. AOCI 的核心思想与功能到底是什么？
2. ARCHMELD 是否缺少这类能力；缺的是哪一种？

结论先说：**AOCI 不是代码分析器，它是“模型作者、受治理、可持久化的系统
认知层”。ARCHMELD 的结构化代码分析更强，但确实缺少这一层。** 这不是
“再建一个 AOCI”的问题，而是 ARCHMELD 目前只有“证据”，没有“理解”的
持久载体。

## 1. 一句话区分两种能力

| 能力 | 回答的问题 | AOCI | ARCHMELD |
|---|---|---|---|
| 结构化代码分析 | 谁调用谁、改这里影响谁、源码在哪一行 | 基本没有（不解析） | 强：快照、图、impact、source、runtime |
| 语义认知 | 这个对象为什么存在、负责什么、改它必须同时考虑什么、什么不能破 | 核心能力 | 弱：brief 是机器汇总，没有持久语义层 |

AOCI 的“分析”是**模型读代码后的综合**，不是解析器分析。它把模型的理解
压缩成每个对象一行，存进 Git，并治理它的更新。ARCHMELD 的解析器能证明
“这条边存在（或只是候选）”，但不能说明“这个模块的责任边界”和“这条
业务约束不能动”。这两件事不是同一种能力，也不互相替代。

## 2. AOCI 的核心思想

### 2.1 认知是工件，不是上下文窗口的状态

AOCI 把“模型对系统的理解”写成 `aoci.txt` / `aoci.code.txt`，与代码一起
提交。换会话、换 Agent、上下文压缩后，认知还在。它解决的不是“这次检索
不到”，而是“每次都要重新建立系统模型”。

### 2.2 模型负责语义，工具负责治理

AOCI 的规范明确禁止程序选择 tag、生成/预填/压缩/改写 F/R/A/S。模型读
证据后写语义；程序只做范围、格式、配额、CAS、基线、恢复、漂移检测。
这是它最重要的边界：**语义的可信度由模型承担，治理的确定性由工具承担。**

### 2.3 一个对象一行，密度优先

FRAS 四字段：

- `F` Function：这个对象负责什么（一个命题，不是清单）。
- `R` Relations：理解或修改它时必须一起考虑的对象（选择性的强关系，
  不是完整依赖图）。
- `A` API：稳定的外部契约（不是所有方法/字段/调用者）。
- `S` Non-obvious constraints：从名字、F、R、A 推不出来、删掉会导致改错
  的高熵约束。

一个文件约 80-150 tokens；70 万行系统的索引约 30 万 tokens。它的价值
来自**省略**：只写模型真正需要记住的东西，而不是把所有边都倒出来。

### 2.4 读在行动之前，维护在完成之后

AOCI 的 AGENTS 契约要求每个新 Agent Run 先建立完整认知，再开始业务任务；
改完代码后 `aoci_maintain` 检查 Missing/Orphan/Stale/Unbaselined。它把
“用不用认知层”变成任务生命周期的一部分，而不是可选查询。

### 2.5 地图是索引，不是知识图谱

AOCI 明确说它不是 knowledge graph，也没有 graph/vector database。`R`
是路径级强关系，不是解析后的 call edge；派生投影必须标 `derived=true`，
不得冒充权威。它选择“够用的强关系”，而不是“完整的图”。

### 2.6 受治理的增量维护

Baseline、CAS、Ledger、Recovery、Scope Change、配额、原子写：地图不是
随手改的缓存，而是像代码一样有基线、有收据、有回滚。维护是增量的，
不是每次重新生成；重新生成会丢掉模型的理解。

### 2.7 证明读过（Attestation）

`aoci_overview` 分块交付，Challenge/Attestation 要求模型按 Entry 序号
回答，证明它真的读了完整索引；没有通过就只能声明“已加载/已交付”，不能
声明“已理解整个系统”。这是对“模型声称读过”的反制。

### 2.8 本地、语言无关、互补

不联网、不上传、不读业务数据；索引来自 Git 清单；代码、测试、配置、
文档、数据库表都可以是对象。它明确建议和 CodeGraph 一起用：地图知道
系统，CodeGraph 取准确代码。

## 3. AOCI 的功能拆解

### 3.1 九个 MCP 工具

| 工具 | 作用 | 在 ARCHMELD 里的对应/缺口 |
|---|---|---|
| `aoci_rules` | 返回当前版本的会话契约 | 规则是静态 AGENTS.md；没有动态 contract 工具 |
| `aoci_overview` | 分块交付完整索引 + Challenge/Attestation | `brief` 只有一屏；没有完整系统视图/证明 |
| `aoci_get_entries` | 按对象读取 Entry | `query`/`source` 更精确，但没有语义 Entry |
| `aoci_search` | 按概念搜索 Entry | `query` 搜符号/文本，不搜责任/约束 |
| `aoci_maintain` | 检测 Missing/Orphan/Stale/Unbaselined | `review` 检测字节/实体变化，不检测语义陈旧 |
| `aoci_update_entry` | 提交完整语义更新批次 | 没有语义写入面 |
| `aoci_remove_entry` | 受治理地移除 Entry | 没有语义删除面 |
| `aoci_header` | 管理索引头/卷 | 没有对应 |
| `aoci_report` | 证据不足时记录后续工作 | `unknowns` 是 per-brief，不持久 |

### 3.2 FRAS 与索引结构

- Root Manifest 声明 Meta/Code/Database Volumes；每个 Volume 是普通文本。
- Section header 记录目录；Entry 是 `name[tag]: F | R | A | S`。
- Tag 是 A/B/C/D/E 轴（层/域/重要度/技术特征/规模），来自 Meta 字典。
- Scope 把对象分为 `index`/`observe`/`exclude`；预算控制整图大小。
- Baseline 记录每个 Entry 对应的源摘要；漂移按路径比较。

### 3.3 治理与写入

- 每次正式写是 receipted CAS 事务，Baseline 最后写；无法证明的字节不覆盖。
- 写入结果只有 `applied`/`repair_required`/`stopped`；`stopped` 不是成功。
- Scope Change 需要人批准；模型不能自批。
- 恢复/回滚有 journal；并发 writer 有锁。
- “全绿”只证明结构与治理条件，不证明语义正确。

### 3.4 读取与认知状态

- `delivery_verified`（已交付）→ `cognition_verified`（Attestation 通过）
  → `cognition_governed`（治理对齐）。
- 上下文压缩后必须重新完整加载；Attestation 不通过只能声明“已加载”。
- 这套状态机很重，但解决了一个真实问题：模型可以假装读过。

## 4. ARCHMELD 现在有什么

- 不可变快照 + SQLite 证据账本 + CAS 对象。
- 静态图：实体、关系、basis/resolution/evidence_ids/conditions。
- 查询：symbols/text/impact/neighbors/path，分页游标。
- 源码：快照内精确字节范围（行级 anchor）。
- 变更：review-diff、affected entities/relations、coverage。
- 运行时：Perfetto/Tracy 导入、span→source、observed relation。
- 架构候选：CodeBoarding 导入，候选组件，不冒充已确认架构。
- 语义/构建：LSP、SCIP、七语言验证、构建/重命名/Changeset。
- 入口：`init`、MCP 8 工具、brief、session card、AOCI 只读 interop。

它擅长回答：**“这条关系/这行代码/这次改动是什么状态？”**
它不擅长回答：**“这个系统为什么这样组织、这个对象的责任边界是什么、
改它必须同时考虑什么、哪条业务约束不能破？”**

## 5. ARCHMELD 缺的是哪一种能力

### 5.1 不缺结构化分析

解析、符号、调用、影响、源码范围、runtime、构建验证，ARCHMELD 都比
AOCI 强。AOCI 不做这些，也不打算做。用 AOCI 替换 CodeGraph/ARCHMELD
是错误方向。

### 5.2 缺“语义认知层”

具体缺口：

1. **没有持久、Agent 可读的系统说明。** brief 是每次现算的一屏 JSON/
   markdown；换会话就没了。`SESSION.md` 是投影，不是系统语义。
2. **没有“为什么/责任/约束”字段。** 图能说 `checkout -> apply_discount`
   是 candidate，但不能说“这个折扣是促销规则，发票和 analytics 假设 0.9，
   改它要同步改三处”。M2.5-D 的 fixture 正是这种约束，图和 grep 都能找到
   调用点，但“哪条假设会破”需要模型综合。
3. **没有语义漂移。** review 能发现文件变了、实体变了；不能发现“Entry
   描述的责任已经不对了”。语义陈旧只有模型重读才能判断。
4. **没有跨会话/跨 Agent 的共享理解。** 账本和图是机器数据；下一个 Agent
   不会自然读到“上次为什么这么改”。handoff 目前靠 brief/context，缺少
   稳定的系统级摘要。
5. **没有语言无关的语义覆盖。** 图主要覆盖代码；配置、文档、运维脚本、
   数据库表的“责任/约束”没有地方放。AOCI 的 file/table Entry 正好补这块。
6. **没有“读过”的证明。** 规则要求 session-start，但没有 attestation；
   模型可以声称读过。这个缺口存在，但不一定要用 AOCI 的重型 Challenge
   来解决。

### 5.3 缺口的本质

不是“分析引擎不够强”，而是**分析结果没有被压缩、持久化、治理和复用**。
ARCHMELD 把证据做得很扎实，但每个会话都从证据重新推导理解；AOCI 把
理解本身变成仓库资产。两者结合才是完整链路：

```text
证据（ARCHMELD） → 理解（AOCI 式认知层） → 行动（Agent/Changeset）
```

## 6. AOCI 在 ARCHMELD 里可以扮演的五种角色

### 角色 1：语义层（E 的核心）

模型作者 file-level Entry，`R` 绑定 ARCHMELD 证据。ARCHMELD 提供
snapshot/entity/relation/source；Entry 提供 F/A/S 和“必须一起考虑”的
强关系。这是 E，不是 AOCI 本体。

### 角色 2：外部伴生工具（已有 C 互操作）

对已经在用 AOCI 的用户：ARCHMELD 只读 `aoci.txt`/`aoci.code.txt`，
`brief(recipe=change)` 列出受影响 Entry；不写、不复制、不依赖其二进制。
这条路成本最低，适合“用户已经有 AOCI 地图”的情况。

### 角色 3：设计参考（治理与维护）

AOCI 的 scope/budget/baseline/CAS/receipt/drift 分类/attestation 是
成熟的设计参考。E 可以借用这些概念，但用 Rust/CLI 的轻量实现，不引入
Go 运行时和 3 个 MCP 写工具。

### 角色 4：迁移来源（AOCI → E）

如果仓库已有 AOCI Entry，E 可以把它们作为 `provider=aoci` 的
**未验证语义**导入：保留 F/A/S 文本，解析 R 的路径引用，标记
`semantic_claims=unverified`，再由 `map check` 逐步绑定到证据。这样
AOCI 用户不必从零建图，ARCHMELD 也不必复制 AOCI 的实现。

### 角色 5：对照与压力测试

AOCI 的失败模式（14 万 tokens/会话、1 小时/20 万行、stale map、宿主
协议摩擦）是 E 的预算/交互设计的反例。E 不应该复刻“每会话强制读完整
索引”，而应该按需读、40k 预算、drift 可见。

## 7. 采纳 / 改造 / 拒绝清单

### 采纳

- 模型作者语义；禁止程序生成/预填 F/R/A/S。
- 一个对象一行的 FRAS 密度。
- Git 版本化、可读、可 diff 的持久工件。
- Scope/budget/observe/exclude。
- Missing/Orphan/Stale/Unbaselined 漂移分类（E 加 Ambiguous）。
- “维护是完成定义的一部分”。
- 本地优先、不上传源码、不读业务数据。

### 改造

- `R`：从路径级散文改为 `file:`/`entity:`/`relation:` 引用，逐条解析。
- Overview：默认按需读整图，超预算分块/游标；不做每会话 14 万 tokens。
- Attestation：不做完整 Challenge/Attestation；用 session card 的
  `map_read`/`map_digest` 做轻量读收据，规则仍以 D 的会话契约为准。
- 写面：CLI `map apply`（候选文件 + 哈希 + CAS + receipt），不进 MCP。
- 治理：轻量 receipt/文件锁/expected-hash，不引入 20 万行 Go 引擎。
- Volume：E1 单文件 `archmeld.map.txt`；超大仓库再按 scope 分卷。
- Tag：最小 `L/D/C/M`；无 Tag 合法；不复制 AOCI 字典。
- 覆盖：E1 先代码文件；配置/文档/DB 表作为 `file:` 对象逐步纳入。

### 拒绝

- 9 个 MCP 工具、3 个写工具。
- 每会话强制完整 overview + 重型 Attestation。
- 把地图当知识图谱/调用图。
- 自动生成语义或“机器草稿 + 模型润色”。
- 复制 AOCI 代码或依赖其 FSL 实现。
- 把 AOCI 的“全绿”当语义正确证明。
- 数据库认知（E5 再议）。

## 8. 对 E 计划的影响

E 计划的方向不变，但定位要改写：

- **E 是补 ARCHMELD 缺失的认知层，不是“反 AOCI”。** AOCI 是设计参考，
  不是竞争对手。
- **E1 的验收不只看“能校验”，还要看“Entry 是否真的承载了图没有的
  信息”。** 如果 Entry 只是把图的关系抄一遍，E 没有价值。
- **E1 接受任何文本对象**（代码、配置、文档、运维脚本），`file:` 引用
  不要求解析器；`entity:` 只在有图时使用。这是 AOCI 语言无关性的
  证据绑定版。
- **E 增加 AOCI 导入路径**：`map import-aoci`（E2/E3）把现有 AOCI Entry
  作为未验证语义导入，逐步绑定证据；不复制 AOCI 代码。
- **E 不做每会话强制完整阅读。** M2.5-A 的规则仍只要求 status+brief；
  map 按需读；大图用 `map status --entries --cursor` 分块。
- **E 的 attestation 降级为读收据**：session card/header 记录
  `map_digest` 与 `map_snapshot`；不实现 Challenge 协议。
- **E 的写入仍是 CLI-only**；这是和 AOCI 最大的产品边界差异。

## 9. 场景示例：AOCI 式认知层在 ARCHMELD 里怎么用

### 9.1 接手项目

1. `archmeld init` 建快照/图。
2. 模型按目录读 `source`/`context`，写 `archmeld.map.txt`（file-level）。
3. 下一次会话：`brief(recipe=project)` + 读 map；`query` 只在需要精确定位时用。
4. 产出：一页系统职责 + 关键约束 + 每条的 refs/unknown。

### 9.2 改动评估

1. `brief(recipe=change)` 列出 changed files/entities。
2. map 关联：哪些 Entry 受影响；它们的 `S` 里有哪些“不能破”的约束。
3. `review` 给出 affected relations；candidate 仍是 candidate。
4. 产出：改动影响 + 约束冲突 + 需要一起改的对象。

### 9.3 模块整理

1. map 的 `F`/`R` 指出职责重叠/强关系。
2. `query impact` 给出真实 caller；`source` 读实现。
3. 形成 refactor 候选；实际改动走 Changeset/apply。
4. 产出：整理建议 + 证据 + 风险。

### 9.4 交接

1. `brief(recipe=map)` 给 map 健康度/stale。
2. 下一个 Agent 读 map + unknowns + next actions。
3. 产出：跨会话、跨 Agent 的稳定上下文，不依赖聊天记录。

## 10. 风险与边界

- **语义错误比没有地图更危险。** 必须 `semantic_claims=unverified`，
  行为证据回 source；地图不能成为“权威”。
- **地图陈旧。** header snapshot + maintain；三次改动无人更新就降级为
  只读提示，不进入强制规则。
- **作者成本。** 1 小时/20 万行是 AOCI 的实测；E 用 file-level + 预算
  控制，但仍不便宜。E4 必须测作者耗时。
- **图关系误绑。** Python 同名 bug 会让 `relation:` 引用错；E1 只做
  file/entity，修复后再开放 relation。
- **模型把地图当答案。** 规则必须写“地图用于系统认知，行为证据回
  source”；地图不能替代 `verification`。
- **多 Agent 冲突。** 单 writer + expected-hash；M4 再做 scoped authority。
- **许可证。** 不复制 AOCI 代码；FRAS 思想公开，文档注明来源。

## 11. 结论

AOCI 的核心不是 Go 代码、不是 9 个工具、不是 14 万 tokens 的索引；它的
核心是：**把模型对系统的理解变成受治理、可持久、可维护的仓库工件**。
ARCHMELD 目前有证据、有图、有 review，但没有这个工件。缺的不是“代码
分析”，而是“分析之后的语义沉淀”。

所以正确的关系不是“AOCI 能不能替代 CodeGraph”，而是：

- AOCI 提供认知层的思想和治理经验；
- ARCHMELD 提供证据、图、源码、runtime 和 review；
- E 把两者结合：模型作者语义，ARCHMELD 校验引用，候选/已解析不混，
  写面 CLI-only，预算 40k，先只读再写入。

E 的计划（`docs/assessments/E-EVIDENCE-COGNITION-PLAN-2026-11-24.md`）
方向不变，但定位从“反 AOCI”改为“补 ARCHMELD 的认知层”；E1 先做
`map check/status`，并把 AOCI 导入作为后续迁移路径。

## 12. 再评估（同日第二轮）：哪些成立，哪些必须收紧

本节是对前 11 节的自我审查，不是复述。先给结论：

1. 主判断成立：AOCI 的核心是“模型作者、受治理、可持久的系统认知层”，
   ARCHMELD 的强项是结构化证据，缺的正是这一层。
2. 但“ARCHMELD 没有理解”说过头了。准确说法是：ARCHMELD 有**查询期
   解释**（`context` 的 claims/counterevidence、relation 的 `conditions`、
   `review` 的 findings），缺的是**持久、按对象归属、可维护**的解释层。
3. “持久文本”不是 E 的价值来源。仓库里的 README/ADR/注释同样持久，
   grep 也能读。E 的价值只能来自三件事同时成立：**变更范围内可寻址的
   选择**、**图推不出的约束**、**引用解析失败会显式报错**。缺任何一条，
   E 都不如直接读源码或文档。
4. E 的价值尚未被证明。E1/E2 的测试全是结构测试；E4 目前的指标（预算、
   作者耗时、编辑前读图比例、candidate 不冒充）仍不能回答“Entry 是否
   承载了图没有的信息”。在证明这一点之前，不应为写入治理投入 E3。

### 12.1 成立的部分（保留）

- 能力二分：AOCI 不解析调用图，ARCHMELD 的 snapshot/entity/relation/
  source/runtime 是真实的强项。
- 治理参考：baseline/CAS/receipt/recovery/drift 分类/attestation 是
  成熟设计；E 借用概念、不复制实现，这是正确的。
- 缺口存在：brief 每次现算；SESSION.md 是投影；账本没有“责任/约束”
  字段；review 能发现字节变化，不能发现语义过期。
- “模型负责语义、工具负责治理”是正确边界；E 的 `R` 解析只约束引用，
  不证明 `F/A/S`。

### 12.2 必须收紧的九个点

**（1）“没有理解”应改为“没有持久、按对象、受治理的理解”。**
`context` 已能承载 claims/counterevidence，`conditions` 已能表达关系
成立条件，`review` 已给出 finding。它们的问题是 per-query、不按对象
归属、不被维护，而不是“不存在”。这个区别决定 E 的对手不是“空白”，
而是仓库里已有的 docs/ADRs/注释。E 必须回答：为什么不直接维护
`docs/ARCHITECTURE.md`？唯一站得住的答案是：按对象、可寻址、每条强
引用可解析/可失败、漂移可见、与 change brief 联动。否则 E 只是又一份
会腐烂的文档。

**（2）“持久文本”不是差异化，“选择性可寻址”才是。**
40k 地图整读比 grep 贵；AOCI 的 14 万 tokens 正是整读的代价。E 的
working loop 必须是：`brief(change)` 选出受影响的少数 Entry →
`inspect(map_entry)` 读那几条 → `source` 验证行为。地图的大部分内容
在单次任务中不应该被读。E2 已经具备这个 loop 的零件，但文档还没有把
“选择”而不是“持久”写成价值主张。

**（3）“14 万 tokens/会话”是对手的选择，不是普遍失败。**
AOCI 用固定全局阅读换确定性；E 用按需阅读换低成本，但承担“没人读”
的风险。低成本本身不是价值；必须证明读 map 改变了决策。否则 E 只是
“更便宜的没人用的东西”。

**（4）Attestation 缺口比原文小，但“收据 ≠ 理解”要写清。**
M2.5-A 已要求最终答复带 snapshot/counts，这是轻量收据；session card
可以记录 `map_digest`。它证明“看到了当前数据”，不证明“读了/用了
map”。中间选项不是 AOCI 的重型 Challenge，而是规则 v3 的轻量引用
要求：改动文件有 Entry 时，最终答复要么引用该 Entry，要么明确说
“未参考”。这比 Challenge 便宜，且可直接从 transcript 检验。

**（5）“跨会话共享理解”混淆了当前语义与历史。**
map 的 `S` 是当前约束（“invoice 假设 0.9”），不是“上次为什么改”。
后者属于 commit/ADR。把 map 当 changelog 会让它无限膨胀并腐烂。
E 的边界应写成：map 记录当前状态语义；历史链接 commit/ADR，不复制。

**（6）AOCI → E 的导入风险被低估。**
AOCI Entry 的 `F/A/S` 是在 AOCI 基线上写的，`R` 是未解析路径。导入后
路径可能解析成功，制造“已绑定证据”的假象。建议：

- 保持 C 的只读外部视图（`provider=aoci`），不并入正式 map；
- 若必须导入，标 `origin=aoci`、`reauthor_required=true`，
  且 `map apply` 不接受未重写的 Entry；
- `map import-aoci` 不进 E3 的必做项，等 E4 证明需求再做。

**（7）非代码对象的“证据绑定”更弱。**
`file:` 引用只能证明文件存在、blob 未变；不能证明 `F/A/S`。E 应输出
`binding: file|entity` 的计数，并在 markdown 里说明“entity 引用有图
证据，file 引用只有字节证据”。否则会把“文件哈希绑定”说成“语义证据
绑定”。

**（8）9 工具 vs 8 工具不是同类比较。**
AOCI 是 6 读 + 3 写；ARCHMELD 是 8 读 + CLI 写。正确的比较是
“读面/写面/写入治理”，不是数量。E 的 MCP 只读不是缺陷，而是产品
边界；代价是维护动作在 MCP 宿主里不可达，必须在 E4 测维护依从性。

**（9）E 的价值未被证明。**
E1/E2 测试覆盖解析、孤儿、陈旧、预算；没有一个测试证明 Entry 承载了
图没有的信息。E4 的现有指标也不测结果。需要把 E4 改成“语义价值
实验”，否则 E3 的 CAS/receipt 可能是在给没人读的地图做治理。

### 12.3 收紧后的价值主张

一句话：

> 在变更范围内，用最小可寻址的语义条目说明：哪些对象受影响、它们
> 承担什么责任、改它必须同时考虑什么；每条强引用都能在当前快照解析，
> 解析失败就显式报错；地图不替代源码，不是调用图，不是通过证明。

三个必要条件是并列的，缺一不可：

- **选择**：change brief 只列 affected，不倒整图；
- **增量**：Entry 写图推不出的责任/约束，不抄 caller 列表；
- **可失败**：refs 解析失败/歧义/陈旧必须让 `map check` 变红。

如果 E4 只能证明“地图存在且便宜”，不能证明“读地图改变了决策”，
E 应停在 E2 只读，不进入规则，不推广 E3。

### 12.4 E4 重新设计：先证明价值，再建写入治理

原计划 E3（apply）→ E4（pilot）。建议改为 **E4a → E3 → E4b**：

**E4a：约束种子实验（不需要 E3）**

手写一份最小地图即可，地图是普通文本，`map check` 可校验。fixture：

- `pricing.py::apply_discount` 返回 `value * 0.9`；
- `invoice.py` 和 `analytics.py` 各自硬编码 `0.9`；
- map 的 pricing Entry `S` 写：“invoice 和 analytics 假设 0.9；改折扣
  必须同步这两处。”
- 任务：“把折扣从 10% 改成 15%”。
- A 臂：`brief` + `source` + grep，无 map；B 臂：`brief(change)` 列出
  Entry，规则要求编辑前读 Entry。
- 记录：是否同步修改 invoice/analytics、是否引用 `S`、tokens、是否把
  candidate 说成 confirmed、是否出现无依据结论。
- Go：B 在 ≥2/3 次运行中避免“只改 pricing”的错误，或明确引用 `S` 并
  同步两处；tokens ≤ A 臂 1.3 倍；0 candidate 冒充。
- No-go：A/B 结果无差异，或 B 只是多读了一段文本但决策不变 → 停在 E2。

这个实验直接测“图/grep 找不到、只有 map 知道”的约束，是对 E 论题的
证伪测试，而不是预算测试。

**E3：最小 apply（E4a go 后）**

- `map apply --candidate --expected-hash --actor`；
- 校验 header snapshot、refs、confirmed 候选边、secret、并发；
- 原子写 + receipt（`map_digest`/`previous_digest`/snapshot/actor/time）；
- 不做 journal/恢复状态机，除非真实并发证明需要；
- `import-aoci` 不进 E3 必做项。

**E4b：规模与维护（E3 后）**

- 50-100 文件仓库建图；记录 tokens/作者耗时/check 错误/stale 检测/
  binding 分布；
- 连续 3 次会话观察维护是否发生；不维护就降级为只读提示；
- 抽样 `F/A/S` 分类：复述图事实 / 图外洞察 / 错误；要求至少有一定
  比例的图外洞察，错误可解释。

### 12.5 证伪条件（任一成立即停/降级）

1. E4a 无结果差异：读 map 不改变决策 → 停在 E2。
2. 抽样中 >50% Entry 只是复述图事实 → E 没有提供图外信息。
3. 连续 2/3 次改动无人 `map maintain` → 地图必然腐烂。
4. 任何一次 `map check` 绿色被当作“架构已验证/测试通过” → 标签失败，
   先修标签与规则，不推广。
5. 地图 >40k 或作者成本不可接受 → 缩 scope，不静默截断。
6. 导入的 AOCI Entry 被当成已绑定证据 → 停止导入，保留只读外部视图。

### 12.6 代码审查发现的两个“绿色陷阱”（本轮已修）

写这份再评估时对照实现，发现两处与 ADR/计划不一致：

1. **`semantic_claims` 没有输出。** ADR-058 和 E 计划都要求
   `semantic_claims=unverified`，但 `check_map`/`entry_report`/`summarize`
   从未输出该字段。一个绿色的 `map check` 因此可能被读成“语义已验证”。
   本轮在 `check_map`/`entry_report`/`summarize`/`maintain` 和 markdown
   中加入 `semantic_claims: unverified`、`verification: not_run`。
2. **`brief(recipe=map)` 的顶层 `state` 是 brief 状态，不是 map 状态。**
   map 为 `invalid`/`stale` 时，顶层仍返回 `state: ok`（`map.state` 才
   是真实健康度）。本轮增加顶层 `map_state`，避免“brief ok = map ok”。
3. **`summarize` 只解析 header，不解析 refs。** `status.map.state=ok`
   只表示 header 与快照匹配，不表示 refs 可解析。本轮增加
   `validation: not_run`，把“未校验”显式化。

这三处都属于“防止错误认知被固化”，比新增功能优先。

### 12.7 结论（第二轮）

AOCI 的核心判断成立，但 E 的卖点必须从“持久文本”改成“变更范围内的
选择性、证据绑定、可失败”。E 的最大风险不是技术，而是**没人读、
没人维护、被当成已验证**。因此顺序应是：先补标签（本轮），再做
E4a 语义价值实验；只有 E4a 证明读 map 改变决策，才做 E3 写入治理，
最后才做 E4b 规模与维护。否则 E 会变成 AOCI 已经证明会失败的那类
工件：很完整、很贵、没人用。
