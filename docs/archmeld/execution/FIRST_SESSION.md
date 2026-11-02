# 本地 Agent 第一轮执行手册

目标：复用固定上游，完成实际基线与第一条真实索引路径，而不是继续讨论产品方向。预计耗时依机器与依赖条件而变，本包不预承诺时间。

## 0. 工作区选择与许可

本初始化包为只读基线；在旁边创建 `<repo>`。不要把工作台开发仓库建在待分析的用户项目中。先读取本包 AGENTS.md 和固定 ADR。用户批准开发范围后，一般构建和测试无需反复确认；真实硬件、源码外传、新付费服务和越权安装须另批。

## 1. 固定源码（PowerShell）

下面只克隆并检出上游，不运行其项目代码。执行前检查工具与目标目录；失败立即停止本步骤，不能继续制造“成功”记录。

```powershell
$ErrorActionPreference = 'Stop'
$Base = '590c2ca7dd7b37bd1d41caf45fac535b6522f969'
$Dest = Join-Path (Get-Location) '<repo>'
if (Test-Path $Dest) { throw "目标已存在：$Dest。不要覆盖，先核实是否为本项目。" }
git clone --no-checkout https://github.com/codegraph-ai/CodeGraph.git $Dest
if ($LASTEXITCODE -ne 0) { throw 'clone failed' }
Set-Location $Dest
git switch --create archmeld/initial $Base
if ($LASTEXITCODE -ne 0) { throw 'checkout failed' }
$Head = (git rev-parse HEAD).Trim()
if ($Head -ne $Base) { throw "错误基线：$Head" }
git remote rename origin upstream
if ($LASTEXITCODE -ne 0) { throw 'remote rename failed' }
git status --short
```

POSIX 对应命令：

```sh
set -eu
base=590c2ca7dd7b37bd1d41caf45fac535b6522f969
test ! -e <repo>
git clone --no-checkout https://github.com/codegraph-ai/CodeGraph.git <repo>
cd <repo>
git switch -c archmeld/initial "$base"
test "$(git rev-parse HEAD)" = "$base"
git remote rename origin upstream
git status --short
```

**不要现在推送远端。** 后续需要远端由用户指定。不要删除原仓库的LICENSE/NOTICE、测试或配置。把本规格复制到 `docs/archmeld/`；保留上游已有AGENTS.md并审阅合并，不直接覆盖。新增根入口可以指向 `docs/archmeld/AGENTS.md`。

## 2. 工具链与上游原样基线

记录 OS/CPU/RAM、Git、rustup、rustc、cargo、Node/npm、C/C++编译器、libclang和构建依赖。上游 rust-toolchain.toml 已固定 Rust 1.94.1；确认来源及实际安装，不自行改 stable。Node选择24系列的实际可用补丁并锁定；不能仅记录“最新版”。

审阅 Cargo.toml/Cargo.lock/build scripts 和 vscode/package.json。执行项目构建会运行构建脚本，须在批准的开发环境。以下为上游已存在的构建入口形式，不是新产品命令：

```text
cargo metadata --locked --format-version 1 --no-deps
cargo test -p codegraph --locked
cargo test -p codegraph-parser-api --locked
cargo build -p codegraph-server --release --locked
cd vscode
npm ci --ignore-scripts
npm run esbuild
```

若上游没有 npm lock，先按B001审阅并解析依赖、生成lock，不运行无效 `npm ci` 后假装完成。`--ignore-scripts` 初始安装不代表构建已安全：显式构建仍会执行代码；需要的postinstall/native步骤逐项审阅后运行，保留实际日志。Rust原生依赖缺失须定位，并按平台安装许可处理。

把每条命令、cwd、工具版本、开始结束、退出码、stdout/stderr保存到 `baseline/`（实际路径由开发仓库约定）。禁止把初始化包 validation/RESULTS 当成这些产品测试。

## 3. 三个必做源码探测

首先定位 `crates/codegraph/src/export/json.rs`、图存储接口、`codegraph-server` 索引初始化与 `vscode` 启动流程。测试：删除中间ID再导出是否漏对象；不同快照和项目是否能隔离cache；graph-only是否仍下载/遥测。记录源码位置、最小输入、实际输出，不凭README推断。

若发现缺口，仅做薄桥接补丁：真实ID迭代、隔离state root、分批导出来源。不重写解析器，不升级到最新上游逃避定位。已检查的导出循环是风险线索，不是已证明bug。

## 4. 开始第一纵向切片

依B003–B007建立：真实仓库快照 → 既有解析器索引 → 带来源证据批次 → SQLite事务 → 有界查询 → 编辑器源码与图节点互定位。先实现该链，再扩展语义、runtime和完整重构。不要先做账户、云服务、向量平台、插件商店或通用多Agent调度。

建议首批提交主题：`baseline: pin upstream and record probes`；`core: snapshot and evidence transaction`；`bridge: isolated structural export`；`workbench: source and evidence navigation`。提交内容和测试须实际存在，不预先生成虚假完成记录。

## 5. CodeBoarding 获取（进入B017时）

独立缓存目录克隆 `https://github.com/CodeBoarding/CodeBoarding.git`，检出 `a38a8d157150e78cee4e4c0f3f0646a4ae9ed938`。审阅 pyproject.toml、uv.lock、TELEMETRY.md 和工具下载行为，再创建隔离Python环境。没有模型授权时不发请求；其输出仅导入candidate。无需第一天把其整套环境混进主工程。

## 6. 会话交接

每个完成任务更新 work-items 的本地执行副本和 PROGRESS，保留本包原始 not_started 基线。遇到失败先定位；确有外部阻碍标 BLOCKED_EXTERNAL，继续不受阻任务。不要用“建议下一步测试”结束已经能测试的步骤。第一天不以R1命名V0。
