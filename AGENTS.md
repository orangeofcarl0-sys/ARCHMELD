# AGENTS.md — ARCHMELD 工作仓库入口

本仓库是 CodeGraph 固定源码（`ae4b2e54fc8ce1c074523943fc750b20f68bf8aa`，0.21.2）的薄分叉。
上游没有任何 `AGENTS.md`，因此不存在需要合并的上游版本；本文件即根入口。

**权威约束在 [`docs/archmeld/AGENTS.md`](docs/archmeld/AGENTS.md)，必须完整阅读并遵守。**
规格、契约、验收与任务清单位于同一目录树下：

| 路径 | 内容 |
|---|---|
| `docs/archmeld/AGENTS.md` | 构建约束（权威） |
| `docs/archmeld/START_HERE.md` | 读取顺序与首次会话要求 |
| `docs/archmeld/docs/00_BLUEPRINT.md` … `03_ARCHITECTURE_DECISIONS.md` | 方案、底座、复用地图、ADRs |
| `docs/archmeld/specs/` | S01–S19 详细规格 |
| `docs/archmeld/contracts/` | JSON Schema、OpenAPI、MCP、状态机、适配器协议 |
| `docs/archmeld/acceptance/` | 需求与 AT 用例（本地执行副本状态另行维护） |
| `docs/archmeld/execution/work-items.json` | B000–B032 任务图 |
| `docs/archmeld/templates/PROGRESS.md` | 进度交接格式 |

## 构建环境解析

本机没有 MSVC 链接器与 Windows SDK，使用上游固定的 Rust 1.94.1 的 GNU host。工具链位置不写死在代码里，由 `tooling/portable_env.py` 解析：

```sh
export ARCHMELD_TOOLS_ROOT=<tools-root>
export ARCHMELD_TEMP=<temp-root>
# 可选：MinGW 运行时目录；也可由 gcc -print-file-name=include 自动发现
export ARCHMELD_STRAWBERRY_BIN=<mingw-bin>
cargo +1.94.1-x86_64-pc-windows-gnu <command> --locked
```

- 解析顺序：显式参数 → `ARCHMELD_*` → 标准环境变量 → 仓库同级 `_archmeld_*`（仅当存在）→ 系统默认。`python tooling/portable_env.py --json` 可查看实际解析结果。
- 这是同一上游版本（1.94.1）的不同 host 目标，不改变 `rust-toolchain.toml` 的版本锁定；如需 MSVC 基线须另装 Build Tools 后重测。
- `LIBCLANG_PATH`（libclang 18.1.1，来自 PyPI `libclang`，本地解压目录）仅用于 RocksDB 的 bindgen；不得把该路径写死进产品代码。未设置时由 `ARCHMELD_LIBCLANG_PATH`/`ARCHMELD_TOOLS_ROOT/pylibclang/clang/native` 解析。
- 所有命令通过 `python baseline/run_cmd.py` 执行并追加记录到 `baseline/commands.ndjson`（cwd、argv、退出码、耗时、工具版本、日志哈希）。
- 编译器临时目录由 `ARCHMELD_TEMP` 决定；未设置时回退 `TMP`/`TEMP`/`TMPDIR`，再回退系统临时目录。C 盘空间紧张时显式设置 `ARCHMELD_TEMP`，不要在其上解压工具链或构建产物。

## 结构导航

先读 [`docs/INDEX.md`](docs/INDEX.md)。仓库有四层文档，不要混用：

- `docs/archmeld/` 是**冻结设计包**（`PACK_MANIFEST.json` 封印），描述规格与计划，不代表当前实现状态；任何改动必须同批 `reseal_pack.py --write`。
- `docs/adr/` 是现行决策（ADR-017…061）；ADR-001…016 在冻结包的 `03_ARCHITECTURE_DECISIONS.md`。
- `HANDOFF_CHECKLIST.md` 是当前操作真相；`archmeld-progress.md` 是时间线；`REMAINING_BLOCKERS.md` 是阻塞项。状态陈述应链接，不要复制数字。
- `docs/evidence/` 与 `docs/release-evidence/` 是只追加的执行证据；历史命令路径不得移动。

工具入口以 [`tooling/README.md`](tooling/README.md) 为准：日常测试用 `test_lane.py --lane fast`，证据道用 `--lane evidence`；环境只从 `build_helper.cargo_env()` 取。

## 公开快照纪律

- 公开 origin 只允许 `main` 和 `public-snapshot-*` 分支；本地启用 `.githooks/pre-push`（`tooling/publication_guard.py --install`），CI 同名 workflow 复核。私有分支不得推送。
- 公开仓库只接收 `tooling/public_snapshot.py export` 生成、并通过 `verify-export` 的单提交快照；不得直接 push 私有历史。
- 本地路径、用户名、远程主机和私有数据集标识由扫描/替换流程处理；私有替换表放在未跟踪的 `.archmeld-private/`。

## 上游改动纪律

- CodeGraph 只接收 release：git tag（如 `vX.Y.Z`）或带 release notes 的版本发布提交；禁止把 `upstream/main` 等分支头作为更新来源或直接合并。无 tag 的 release 提交（0.21.2）必须在 `upstreams.lock.json` 显式记录例外。
- 保留上游 `LICENSE`、`NOTICE`、测试与配置；不删除失败测试。
- 每处上游改动写入 `UPSTREAM_PATCHES.md`（必要性、路径、是否回馈 upstream、回归 AT）。
- 不运行 `git reset --hard` / `git clean -fd`；不覆盖用户的未提交改动。
- 新增 ARCHMELD crate 前不改动上游解析器与图 API。
