# S05 · 多语言能力与实现路径

## 1. 能力而非语言数量

每个语言包报告独立维度：structure、definition_reference、call_resolution、runtime_linkage、refactor_actions、build_verify。各维度状态为 unavailable、configured、verified、degraded；包含具体工具、配置、版本、测试编号和限制。运行环境缺失不能仍显示 verified。

CodeGraph 提供结构覆盖；SCIP 提供持久符号索引；LSP/原生 API 提供当前语义查询和变换。SCIP 不是 CFG/PDG，也不保证所有引用是调用。全仓 CFG/PDG 不作为 R1 前置条件；影响分析遇到动态或未解析边必须保守扩展范围并提示风险。

## 2. 语言实施表

| 语言 | 定义/引用首选 | 变换与验证 | 重要边界 |
|---|---|---|---|
| C/C++ | scip-clang（支持平台）+ clangd | clangd rename/Clang 工具；CMake/CTest 或项目命令 | compile_commands、宏、重载、模板、函数指针 |
| Python | scip-python + Pyright 类语义 | LibCST/语言工具；项目 pytest/其他测试 | 导入环境、猴子补丁、装饰器、动态属性 |
| JS/TS | scip-typescript + TS language service | TS rename/refactor/ast-grep；项目 build/tests | tsconfig、路径别名、JS 隐式配置、bundler |
| Java | scip-java + JDT 语言能力 | 原生 rename、已审查 recipe；Maven/Gradle tests | classpath、生成代码、DI 与反射 |
| C# | scip-dotnet + Roslyn/csharp-ls | Roslyn refactor；dotnet build/test | solution、目标框架、源生成器 |
| Go | scip-go + gopls | gopls/Go AST；go test/race 适用场景 | build tags、接口调用、cgo |
| Rust | rust-analyzer 索引/语言能力 | assists/受限补丁；cargo test/clippy | cfg/features、宏、build.rs/proc macro 执行 |

这些是指定集成路径，不是已完成实测的宣传。[SRC-SCIP] [SRC-SCIP-CLANG] [SRC-SCIP-PY] [SRC-SCIP-TS] [SRC-VSC-LANGUAGE]

## 3. 每语言首版最低真实验收

至少一个含多个模块的金标 fixture、一个真实项目样本。验证：定义/引用范围准确；同名/遮蔽/重载不串联；未解析关系明确；跨文件 rename 或等价受控变换不改字符串/无关同名符号；构建测试在变更前后真实执行；一个运行场景关联回正确源码。

Runtime 不要求七套自制 profiler。Python 使用 VizTracer；C/C++ 使用 Tracy 或明确探针；其他语言可通过现成 OTel SDK 或原生工具输出在通用 importer 中验证。只有真正采集并导入才算 runtime_linkage verified。不能拿一份手造通用 Trace 代替七语言实测。

## 4. 平台策略

Windows C/C++ 原生用 clangd 配置准确导航与 rename；需要全量 SCIP 时在明确 Linux/WSL worker 处理兼容工程。Windows-only MSVC 项目不能假装能原样在 WSL 构建，必须保留原生工具链和对应覆盖说明。[SRC-SCIP-CLANG]

所有语言包首次安装由 core doctor 输出依赖与信任要求。不会开启仓库自动安装脚本。编译型项目可能执行 build plugins、annotation processors、proc macros，必须被执行信任策略覆盖。

## 5. 跨语言边界

首版至少实现一条真实 Python↔C/C++ binding/FFI 金标，以及一条 HTTP/消息协议 producer-consumer 场景。静态关系基于注册表、IDL/OpenAPI、路由/绑定配置等明确依据；Runtime 基于 trace/span/message ID。仅名称、URL 字符串相同不构成因果证据。

无法解析的边界作为 `boundary_unresolved` 显示，用户可人工声明意图；人工声明仍不同于运行已证实。生成代码保存 generated_from，允许从生成物跳回协议源，但自动编辑优先源文件，不能直接改生成产物后忘记再生成。

## 6. 可选研究增强

Joern、全量数据流和专用科学语言适配作为按需能力。R1 的所有权/状态判断可以结合源码、语义索引、有限定向切片与运行探针完成，但必须标识推断与盲区。不得为了宣传深度把动态语言的任意执行关系伪装为精确静态结果。
