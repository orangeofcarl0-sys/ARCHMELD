# AGENTS.md — ARCHMELD 工作仓库入口

本仓库是 CodeGraph 固定源码（`590c2ca7dd7b37bd1d41caf45fac535b6522f969`）的薄分叉。
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

## 本机（Windows）构建说明

本机没有 MSVC 链接器与 Windows SDK，使用上游固定的 Rust 1.94.1 的 GNU host：

```sh
export LIBCLANG_PATH='<workspace>\_archmeld_tools\pylibclang\clang\native'
cargo +1.94.1-x86_64-pc-windows-gnu <command> --locked
```

- 这是同一上游版本（1.94.1）的不同 host 目标，不改变 `rust-toolchain.toml` 的版本锁定；如需 MSVC 基线须另装 Build Tools 后重测。
- `LIBCLANG_PATH`（libclang 18.1.1，来自 PyPI `libclang`，本地解压目录）仅用于 RocksDB 的 bindgen；不得把该路径写死进产品代码。
- 所有命令通过 `python baseline/run_cmd.py` 执行并追加记录到 `baseline/commands.ndjson`（cwd、argv、退出码、耗时、工具版本、日志哈希）。
- 编译器临时目录必须留在 F 盘：记录器默认把 `TMP`/`TEMP`/`TMPDIR` 指向 `<workspace>\_archmeld_temp`（可用同名环境变量覆盖）。C 盘空间紧张时不要在其上解压工具链或构建产物。

## 上游改动纪律

- 保留上游 `LICENSE`、`NOTICE`、测试与配置；不删除失败测试。
- 每处上游改动写入 `UPSTREAM_PATCHES.md`（必要性、路径、是否回馈 upstream、回归 AT）。
- 不运行 `git reset --hard` / `git clean -fd`；不覆盖用户的未提交改动。
- 新增 ARCHMELD crate 前不改动上游解析器与图 API。
