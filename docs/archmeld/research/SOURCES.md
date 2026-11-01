# 来源登记与核查边界

核查日期：2026-10-30。只用官方仓库、规范或官方文档作为实现事实来源。URL 指向上游，不代表打包了其源码；固定源码见选择记录。

## SRC-CG-PIN · CodeGraph 固定提交

https://github.com/codegraph-ai/CodeGraph/commit/590c2ca7dd7b37bd1d41caf45fac535b6522f969

GitHub API 已读取 commit；Rust 工具链固定改动可见。不是本地编译结果。

## SRC-CG-MANIFEST · CodeGraph Cargo 工作区

https://github.com/codegraph-ai/CodeGraph/blob/590c2ca7dd7b37bd1d41caf45fac535b6522f969/Cargo.toml

读取工作区、crate 列表、0.21.0 版本与 Apache-2.0 声明。

## SRC-CG-LIB · CodeGraph 图库公开类型

https://github.com/codegraph-ai/CodeGraph/blob/590c2ca7dd7b37bd1d41caf45fac535b6522f969/crates/codegraph/src/lib.rs

读取公开模块/类型与 storage backend 边界。

## SRC-CG-EXPORT · CodeGraph JSON 导出源码

https://github.com/codegraph-ai/CodeGraph/blob/590c2ca7dd7b37bd1d41caf45fac535b6522f969/crates/codegraph/src/export/json.rs

读取导出遍历实现。稀疏编号完整性是待实测风险，不宣称已证实故障。

## SRC-CG-VSCODE · CodeGraph VS Code 扩展清单

https://github.com/codegraph-ai/CodeGraph/blob/590c2ca7dd7b37bd1d41caf45fac535b6522f969/vscode/package.json

读取扩展身份、命令与编辑器最低版本声明。

## SRC-CG-README · CodeGraph 官方说明

https://github.com/codegraph-ai/CodeGraph/blob/main/README.md

graph-only、MCP/LSP、原生引擎、后端缓存和构建入口；宣传性能未作为本产品实测。

## SRC-CG-LICENSE · CodeGraph 许可证

https://github.com/codegraph-ai/CodeGraph/blob/main/LICENSE

根许可证为 Apache-2.0；实际分发仍需检查传递依赖。

## SRC-CB · CodeBoarding 引擎与 CLI

https://github.com/CodeBoarding/CodeBoarding

官方 README 的能力、CLI、遥测和许可说明；固定研究 SHA 见选择记录。

## SRC-CB-PIN · CodeBoarding 固定提交

https://github.com/CodeBoarding/CodeBoarding/commit/a38a8d157150e78cee4e4c0f3f0646a4ae9ed938

GitHub API 已读取 main 的提交；未在本环境构建。

## SRC-SCIP · SCIP 协议与生态

https://github.com/scip-code/scip

官方索引协议及各语言实现入口；不等于完整运行/数据流分析。

## SRC-SCIP-PROTO · SCIP 数据模型

https://github.com/scip-code/scip/blob/main/scip.proto

符号、出现位置与关系定义；导入需处理源码位置编码。

## SRC-SCIP-INDEXERS · 精确代码导航索引器列表

https://sourcegraph.com/docs/code-navigation/precise-code-navigation

多语言工具发现入口；实际版本和运行条件在本地锁定。

## SRC-SCIP-CLANG · scip-clang 平台与编译数据库

https://github.com/sourcegraph/scip-clang

官方预编译平台列 Linux x64 与 macOS arm64；Windows 不直接假设有预编译包。

## SRC-SCIP-PY · scip-python 说明与许可

https://github.com/sourcegraph/scip-python

基于 Pyright 的 Python SCIP 工具；默认分支不一定为 main。

## SRC-SCIP-PY-LICENSE · scip-python 许可证

https://github.com/sourcegraph/scip-python/blob/scip/LICENSE.txt

读取 MIT 许可，不把所有 SCIP 项目统称同一许可证。

## SRC-SCIP-TS · scip-typescript

https://github.com/sourcegraph/scip-typescript

JS/TS 索引与 Node 版本要求；本地按实际选定版本锁定。

## SRC-CLANGD · clangd 编译参数

https://clangd.llvm.org/design/compile-commands

分析需要正确编译命令和构建上下文。

## SRC-VSC-WEBVIEW · VS Code Webview API

https://code.visualstudio.com/api/extension-guides/webview

扩展视图与消息机制；产品必须另做权限/CSP。

## SRC-VSC-LANGUAGE · VS Code 语言能力 API

https://code.visualstudio.com/api/language-extensions/programmatic-language-features

原生符号、引用、rename、code action 等能力复用入口。

## SRC-VSC-TRUST · VS Code Workspace Trust

https://code.visualstudio.com/api/extension-guides/workspace-trust

工作区配置与语言插件可能触发执行；扩展需声明受限行为。

## SRC-VIZ · VizTracer

https://github.com/gaogaotiantian/viztracer

真实 Python 追踪与多种执行模型支持；开销需测量，不能与采样等同。

## SRC-TRACY · Tracy 许可证与工程

https://github.com/wolfpld/tracy/blob/master/LICENSE

BSD-3-Clause 根许可；原生 Trace 与工具版本需配对。

## SRC-PERFETTO · Perfetto 工具与数据分析

https://perfetto.dev/docs/

Trace 处理与可视化基础；产品使用本地资源。

## SRC-PERFETTO-EMBED · Perfetto 嵌入 API

https://perfetto.dev/docs/visualization/embedding-the-ui

官方嵌入方式与消息机制；生产集成需 origin/source 校验。

## SRC-OTEL · OpenTelemetry 追踪概念

https://opentelemetry.io/docs/specs/otel/overview/

Span、上下文与因果 Link；时间接近不等于关联。

## SRC-LIBCST · LibCST codemods

https://libcst.readthedocs.io/en/latest/codemods.html

结构变换、diff 和元数据基础，不保证行为等价。

## SRC-ASTGREP · ast-grep 重写文档

https://ast-grep.github.io/guide/rewrite-code

结构化模式变换；不替代语义重命名。

## SRC-MCP · MCP Tools 规范（已知版本）

https://modelcontextprotocol.io/specification/2025-06-18/server/tools

工具输入输出与 annotations；本地客户端必须协商兼容协议版本。

## SRC-CODEX · OpenAI 官方本地 MCP 接入说明

https://developers.openai.com/resources/docs-mcp

Codex 可连接 MCP 服务；本方案提供服务而非依赖旧的 Codex-as-MCP-server 模式。

## 未完成的验证

本环境不能直接联网克隆和构建上游；没有运行 CodeGraph、CodeBoarding、七语言工具链或产品代码。故未提供虚构二进制 SHA-256、全套依赖 lock、产品性能或测试通过率。两项源码 commit 通过 GitHub 接口确认，源码风险来自实际读取文件。B001/B002 是本地真实构建与能力验证任务，不是允许跳过的研究备注。

部分文档 main 页面会继续更新；采用固定 commit 的内容优先。许可证声明仅描述所查看模块，完整分发清单需本地审查。

## SRC-CYTOSCAPE

Cytoscape.js 官方文档：https://js.cytoscape.org/ 。用于确认交互图基础和 MIT 许可，不证明本工作台已集成。
