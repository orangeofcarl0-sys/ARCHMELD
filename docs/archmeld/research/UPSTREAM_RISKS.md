# 上游风险登记

| ID | 已知依据 / 风险 | 首个验证动作 | 不允许的结论 |
|---|---|---|---|
| U01 | 导出函数按 count 枚举 ID，可能依赖稠密性质 | 插入/删除/恢复后的集合对比 | 未运行就宣布已修复或确定有 bug |
| U02 | 原引擎具全局缓存、模型下载和独立进程入口 | 独立 state root + 禁网 + 文件写入审计 | graph-only 等于所有路径均无副作用 |
| U03 | 语法推导跨文件关系有边界 | 同名/遮蔽/动态调用 goldens | parser 支持等于精确语义 |
| U04 | CodeBoarding 有工具安装/遥测及模型出站 | 最小环境、禁网、出站授权测试 | CLI 开源等于所有发行组件已许可 |
| U05 | SCIP-Clang 官方预编译不覆盖 Windows | 原生 clangd + 显式 Linux worker 测试 | 一条 Linux 命令适合所有 Windows 工程 |
| U06 | 多语言工具可能执行项目代码 | trust/runner 权限与构建脚本检查 | 只叫索引就无需执行许可 |
| U07 | 专业 Trace 转换可能丢字段 | 保存 raw 并验证 loss notes | 统一 schema 意味无损 |
| U08 | upstream HEAD 与公开文档继续变化 | 固定 SHA、记录 patch、升级回归 | main 永远可复现 |

每项在本地写 reproduction、observed、decision、test_refs。未证实不写成事实，未消除则以 limitation/blocked 明示。
