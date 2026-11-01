# S15 · 构建、包装、安装与版本

## 1. 开发基线

CodeGraph 固定 commit 是 Git 根基线，不把其 main 后续变化自动纳入。Rust 使用上游固定工具链文件（本轮读到 1.94.1），新 crate 默认不提高要求。扩展沿用上游 Node 工程并选支持的 Node 24 系列；Python 3.12 用于独立 CodeBoarding 环境。具体 patch、包管理器与全部传递依赖由 B001 实机解析后锁定。[SRC-CG-PIN] [SRC-CG-MANIFEST]

本包没有包含完整工具缓存，因此首次构建需要网络或用户提供已校验离线镜像。文档不得宣称本 ZIP 是开箱即用安装包。

## 2. 从底座产生第一批提交

先做 upstream baseline build/tests；再添加 evidence exporter/cache isolation 的最小 patch；之后引入 core/store 与 adapter；扩展逐步改为调用 core。禁止第一提交搬空现有代码另起一套“更干净”实现。每个上游改动附必要性和回归测试，遵循 docs/02_REUSE_AND_REPO_MAP.md。

## 3. 交付组件

R1 包括 core/CLI 二进制、VSIX、语言能力配置、Trace importer、离线 UI assets、schema 与本地帮助、第三方 NOTICE、工具 lock/SBOM、迁移与卸载说明。专业语言工具可作为独立按需 language pack 安装；核心不能因为用户只用 Python 就要求安装 JDK、.NET 和全套 LLVM。

默认不把大型模型打包；需要 provider 时提示显式配置。扩展不使用原上游 publisher 身份，不覆盖用户已装 CodeGraph 数据。

## 4. 安装与卸载

离线 zip/安装器导入时校验 manifest 和二进制 digest；路径可选择，拒绝写入不属于用户授权位置。自更新默认提示而非静默，升级保留回退安装和数据备份。卸载程序与删除项目证据是两个独立动作，用户未授权不得删项目数据。

## 5. 支持平台

Windows 11 x64、Ubuntu 24.04 x64 为发布实测要求。macOS 可兼容构建但未跑平台 suite 不声明正式支持。WSL worker 与宿主进行严格路径/权限映射，不假装 Windows 原生。目标工程的工具链必须以其实际环境验证，不能只在开发容器运行 toy fixture。

## 6. CI

required jobs：格式/静态、单元、Schema/API 等价、适配器契约、七语言 fixtures、真实项目 smoke、变更故障注入、安全与离线、持久恢复、扩展端到端、packaging、SBOM/license。付费/外部模型不作每次 CI 前提；但 R1 的至少一个真实模型/外部 Agent 接入 smoke 必须有经授权的执行记录。

所有 required Job 失败均阻止 R1 发布。报告单列 product tests 与 spec-pack validator：后者通过不代表实现通过。Release tag 必须绑定源码 commit、toolchain lock、验收报告和产物 hash。
