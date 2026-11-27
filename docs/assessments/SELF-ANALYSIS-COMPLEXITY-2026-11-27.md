# ARCHMELD 自分析与结构收敛

日期：2026-11-27
范围：本仓库的**结构复杂度**，不是产品功能审计，也不是又一次语义自分析（P01–P25 已在
`docs/evidence/self-analysis-fix-2026-11-27/` 闭合）。
本轮只做只读测量与低风险结构整理，不改产品运行时行为。

## 0. 结论

1. **复杂度的大头不在产品代码，而在治理层。** ARCHMELD 自有 Rust 代码约 7.7 万行，占全部
   Rust 的 33%；上游 CodeGraph 约 15.4 万行，占 67%。与此同时，工具脚本 1.2 万行、文档
   1.77 万行、证据 587 个文件，全部由本项目维护。
2. **存在三套并行的“真相”**：冻结设计包（`docs/archmeld/`，138 个文件，仍自称“不是已实现
   产品”）、现行决策与操作文档（`docs/adr/`、`OPERATOR_GUIDE`、`HANDOFF`、`BLOCKERS`、
   `progress`）、执行证据（`docs/evidence/` 523 个文件、`docs/release-evidence/` 64 个文件）。
   三者没有统一入口，也没有明确的有效期。
3. **决策记录被切成两段**：ADR-001…016 在冻结包里，ADR-017…060 在 `docs/adr/`；新读者
   很难知道“全部决策在哪里”。
4. **工具层入口过多、原语重复**：35 个脚本；26 个脚本各自定义仓库根；26 个脚本直接调用
   `subprocess`；至少 13 份 `sha256` 实现；Cargo 环境有 5 层入口；证据 JSON 各自拼装。
5. **产品核心是扁平大模块**：`archmeld-core` 在 `lib.rs` 暴露 43 个 `pub mod`、270 个
   `pub` 项，多个文件超过 1,700 行。真正的代码收敛应在这里，但不能和文档整理混在一次改动里。

本轮已落地的是**第 1 层：把隐式结构显式化**。代码层的收敛列为 T2/T3，需单独验收。

## 1. 口径与方法

- 统计对象：当前 HEAD 的 Git 跟踪文件；排除 `.git/`、`target*/`、`dist/`、`__pycache__/`、
  `_archmeld_*` 等未跟踪产物。
- 方法：`git ls-files`、`wc -l`、Python AST 提取 docstring、按目录/扩展名计数、跨 crate
  引用扫描。
- 限制：行数不等于复杂度；本报告只用它做**结构证据**，不据此判断模块质量。
- 未运行：完整测试道、AT067 证据道、发行打包。本轮没有产品代码改动，不需要重跑验收。

## 2. 度量

### 2.1 仓库规模

| 指标 | 数值 |
|---|---|
| Git 提交 | 416 |
| 跟踪文件 | 1,926 |
| 跟踪内容 | 5.4 MiB |
| Rust | 584 个文件 / 230,903 行 |
| 其中 ARCHMELD 自有（`archmeld-core` + `archmeld-api`） | 76,789 行（33%） |
| 其中上游 CodeGraph | 约 154,114 行（67%） |
| Python 工具 | 35 个文件 / 12,113 行 |
| Markdown | 193 个文件 / 17,660 行 |
| 文档文件总数 | 580（`docs/` 下） |
| 执行证据 | 523 个文件 / 18 MiB |
| 发行证据 | 64 个文件 |
| ADR | 44 个（017–060）+ 16 个（001–016，冻结包内） |
| 冻结设计包 | 138 个文件 |
| 测试 | 2,524 个 `#[test]`，分布在 164 个测试文件 |

### 2.2 最大的文件

| 文件 | 行数 | 所属 |
|---|---:|---|
| `crates/archmeld-core/src/predicate.rs` | 2,867 | 产品 |
| `crates/archmeld-api/src/cli.rs` | 2,614 | 产品 |
| `crates/archmeld-core/src/runtime.rs` | 2,276 | 产品 |
| `crates/archmeld-core/src/changeset.rs` | 2,060 | 产品 |
| `crates/archmeld-core/src/interpret.rs` | 1,913 | 产品 |
| `tooling/upstream.py` | 1,862 | 工具 |
| `crates/archmeld-api/src/lib.rs` | 1,862 | 产品 |
| `crates/archmeld-core/src/apply.rs` | 1,748 | 产品 |
| `crates/archmeld-core/src/verification.rs` | 1,709 | 产品 |
| `tooling/test_upstream.py` | 797 | 工具 |

### 2.3 重复模式

| 模式 | 数量 | 说明 |
|---|---:|---|
| 各脚本自行定义仓库根 | 26 个脚本 | 绝大多数是同一行 `ROOT = Path(__file__).resolve().parents[1]` |
| 直接调用 `subprocess` | 26 个脚本 | 没有统一的超时、环境或错误包装 |
| `sha256` 变体实现 | 13+ 份 | `sha256` / `sha256_file` / `sha256_bytes` 分散在工具与发布脚本中 |
| Cargo 环境入口 | 5 层 | `portable_env` → `build_helper.cargo_env` → `cargo_test` / `test_lane` / `baseline/run_cmd` |
| 状态复述 | 5 个文件 | `HANDOFF`、`REMAINING_BLOCKERS`、`archmeld-progress`、`B030-release`、`B031-release-notes` 都陈述 AT067/B030 状态 |
| 编辑器表面 | 2 个 VS Code 扩展 + 3 个上游表面 | `vscode/`（上游 `codegraph`）、`archmeld-vscode/`（ARCHMELD Workbench）、`vs-extension/`、`jetbrains/`、`mcp-package/` |

## 3. 复杂度来源

### 3.1 三套文档真相，没有统一入口

- 冻结包 `docs/archmeld/README.md` 仍写“不包含生产实现”，与仓库现状冲突；它描述的是
  2026-10-30 的交付包，不是当前 HEAD。
- 现行状态分散在 `HANDOFF_CHECKLIST.md`（操作真相）、`archmeld-progress.md`（时间线）、
  `REMAINING_BLOCKERS.md`（阻塞项）和两个 release notes 中。
- 证据目录按任务与日期累积，没有 README 说明“哪些是当前门禁、哪些是历史”。

**后果**：新会话必须先做一次“哪份文档可信”的研究，再开始工作；这正是交接成本的主要来源。

### 3.2 ADR 被切成 001–016 与 017–060 两段

- 001–016 在 `docs/archmeld/docs/03_ARCHITECTURE_DECISIONS.md`，是表格形式的基础决策。
- 017–060 在 `docs/adr/`，是完整的 ADR 文件。
- 没有一处列出全部 60 条，也没有说明两段的关系。

**后果**：查一个决策要先知道它属于哪一段；新决策容易重复已冻结的基础决策。

### 3.3 工具层入口过多，原语重复

- 35 个脚本中，只有约 10 个是日常入口；其余是任务证据生成器。
- 仓库根、哈希、JSON 读写、子进程调用各自实现；新脚本倾向再复制一份。
- Cargo 环境虽有单一事实来源 `build_helper.cargo_env()`，但上层仍有 5 个入口，文档需要
  同时解释它们的关系。

**后果**：贡献者不知道该用哪个脚本；同一个环境 bug 要在多处修；证据格式继续分叉。

### 3.4 产品核心扁平且文件过大

- `archmeld-core/src/lib.rs` 暴露 43 个 `pub mod`；其中 7 个模块没有被 core 之外的 crate
  直接引用（`anchor`、`cognition`、`language`、`lsp`、`scip`、`source`、`units`），但仍是
  公开 API 的一部分。
- `predicate`、`runtime`、`changeset`、`interpret`、`apply`、`verification` 都超过 1,700 行；
  `archmeld-api` 的 `cli.rs` 超过 2,600 行。
- 模块没有二级分组：证据、变更、运行时、理解、运维全部平铺在同一层。

**后果**：导航靠记忆；改动容易跨多个巨型文件；公开 API 面大于实际需要。

### 3.5 并行界面边界不清

- `vscode/` 是上游 `codegraph` 扩展；`archmeld-vscode/` 是 ARCHMELD Workbench；两者不是
  重复实现，但目录名没有表达归属。
- `vs-extension/`、`jetbrains/`、`mcp-package/` 是保留的上游表面，最后一次改动早于或等于
  上游 pin。
- 这些表面必须保留（上游纪律），但需要在导航里明确“不是 ARCHMELD 的产品入口”。

### 3.6 证据只增不减，没有保留策略

- 20 个日期目录、523 个文件；`archive/` 只有 4 个文件。
- 没有索引说明每个目录对应哪个任务、是否仍被当前门禁引用。
- 删除会破坏历史命令的可复现路径，因此不能简单清理；需要“索引 + 保留策略”。

## 4. 目标结构

原则：**每个关注点只有一个入口；每一层只回答一个问题；历史可以保留，但必须可判别。**

```
产品层      crates/archmeld-core, crates/archmeld-api
底座层      crates/codegraph-*                  （上游，保留许可证与测试）
契约层      docs/archmeld/contracts             （冻结基础）
            contracts/archmeld                  （补丁叠加）
决策层      docs/archmeld/docs/03_*.md          （ADR-001…016）
            docs/adr/                           （ADR-017…060）
操作层      docs/OPERATOR_GUIDE.md
            docs/HANDOFF_CHECKLIST.md
            docs/archmeld-progress.md
            docs/REMAINING_BLOCKERS.md
证据层      baseline/commands.ndjson
            docs/evidence/
            docs/release-evidence/
工具层      portable_env / build_helper / test_lane / upstream / package_release /
            public_snapshot / publication_guard / doctor
```

每个关注点的规范入口：

| 关注点 | 唯一入口 |
|---|---|
| 环境解析 | `tooling/portable_env.py` |
| Cargo 环境 | `tooling/build_helper.py::cargo_env` |
| 日常测试 | `tooling/test_lane.py --lane fast` |
| 证据测试 | `tooling/test_lane.py --lane evidence` |
| 上游更新 | `tooling/upstream.py`（只接受 release） |
| 发行打包 | `tooling/package_release.py` |
| 公开导出 | `tooling/public_snapshot.py` |
| 公开推送保护 | `tooling/publication_guard.py` |
| 工具链健康 | `tooling/doctor.py` |
| 交接状态 | `tooling/handoff_check.py` → `B032-handoff.json` |

## 5. 分阶段收敛

### T1 · 已在本轮落地：导航与边界

新增：

- `docs/INDEX.md`：单一导航入口、权威顺序、四层文档、仓库布局、防漂移规则。
- `docs/adr/README.md`：001–016 与 017–060 的桥接索引，以及新增 ADR 的规则。
- `docs/evidence/README.md`：三类证据存储、命名、20 个日期目录、归档规则。
- `docs/release-evidence/README.md`：门禁文件分组、重跑规则、哪个文件决定哪个声明。
- `tooling/README.md`：35 个脚本的分组、规范入口、约定。
- 本报告。

同时更新根 `AGENTS.md` 与 `docs/HANDOFF_CHECKLIST.md` 的阅读顺序，指向 `docs/INDEX.md`。

**性质**：只增加导航与说明，不改代码、不改证据、不改冻结包。复杂度从“隐式、需要研究”
变为“显式、可索引”。

### T2 · 建议下一步：工具原语收敛（低风险）

目标：把重复的根解析、哈希、JSON、子进程包装收敛到一个 `tooling/common.py`，并迁移
5 个治理脚本（`gen_notices`、`reseal_pack`、`handoff_check`、`publication_guard`、
`doctor`）。

验收：

- `tooling/test_publication_guard.py`、`test_upstream.py` 全绿；
- `reseal_pack.py --check` 通过；
- `doctor.py` 输出与改动前一致；
- 不改变任何证据文件的字节。

**预计收益**：减少 4–6 份重复实现；新脚本不再复制环境逻辑。**风险**：低；迁移脚本需要
保持超时、文本/二进制模式和错误语义完全一致。

### T3 · 产品代码结构收敛（中风险，需单独评审）

按“证据 / 变更 / 运行时 / 理解 / 运维 / 接口”六组重组 `archmeld-core`，先拆最大的四个文件：

1. `predicate.rs` → `predicate/{numeric,invariant,performance,mod}.rs`
2. `runtime.rs` → `runtime/{model,clock,cancel,mod}.rs`
3. `changeset.rs` → `changeset/{prepare,digest,mod}.rs`
4. `interpret.rs` → `interpret/{provider,parse,apply,mod}.rs`

同时把 7 个未被外部 crate 直接引用的模块降为 `pub(crate)`，除非集成测试需要它们。
**验收**：`test_lane.py --lane fast` 全绿；公开 API diff 为空或仅缩小；不改变契约版本。

### T4 · 表面与证据策略（需明确授权）

- 明确 `vs-extension/`、`jetbrains/`、`mcp-package/` 为上游保留面，不投入维护。
- 为 `docs/evidence/` 建立保留策略：当前门禁引用 → 保留；仅历史 → 保留但标记；重复 →
  归档。**不删除**，因为命令复现路径依赖文件名。
- 评估是否把 20 个日期目录的原始大文件移出 Git（保留摘要与命令记录）。这会改变证据
  可复现方式，必须单独决定。

## 6. 明确不做

- 不删除上游 crate、扩展、许可证、NOTICE 或失败测试。
- 不批量移动 `docs/evidence/` 文件：历史命令的 argv 直接引用这些路径。
- 不修改测试期望、数值容差或风险预算。
- 不在 T2/T3 验收前做大规模模块移动。
- 不引入新框架、新依赖或新的 evidence 格式版本。

## 7. 本轮验收

```sh
py -3.11 tooling/reseal_pack.py --check          # 冻结包未变
py -3.11 tooling/doctor.py                       # 工具链锁定仍一致
py -3.11 tooling/test_upstream.py                # 44 项
py -3.11 tooling/test_public_snapshot.py         # 扫描/导出
py -3.11 tooling/test_publication_guard.py       # 12 项
git status --short
```

`handoff_check.py` 会重写 `B032-handoff.json`，本轮不运行；交接状态沿用现有文件。

## 8. 风险与回滚

| 风险 | 缓解 |
|---|---|
| 索引本身漂移，变成第四套真相 | 索引只做导航与链接，不复制状态数字；状态仍由 HANDOFF/progress 决定 |
| T2 迁移改变脚本行为 | 先加自测，逐脚本迁移；每个脚本迁移后跑对应测试 |
| T3 模块移动造成 import 破坏 | 按文件分片，先 `cargo check -p archmeld-core`，再跑 fast lane |
| 证据目录被误删 | 保留策略只允许归档/标记，不允许删除；公开导出由策略控制 |
| 冻结包被误改 | `reseal_pack.py --check` 作为门禁；任何改动必须同批重封 |

## 9. 一句话

**先把“哪份文档算数、哪个脚本是入口、哪次运行算证据”固定下来，再动代码结构。**
本轮完成前者；后者按 T2 → T3 分片推进，每片都有独立验收。
