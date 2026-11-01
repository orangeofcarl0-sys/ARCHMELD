# 实现者需要构建的验收夹具

本包 examples 为契约合成样例，不是以下工程的代用品。不得把样例Trace称为实际采集。

每组语言建立小型金标仓库：至少两个模块、同名符号、跨文件引用、一个无法静态确定的边界、一个异常/取消路径、一个可重放运行场景。运行对应真实工具生成结果；金标期望由实现者审查并固定，不能由同一个待测分析器生成后自比。

| 语言 | 必须包含 | 运行/变更验收 |
|---|---|---|
| C/C++ | 宏双配置、模板、虚函数、函数指针、路径中空格/中文 | 真实编译、Tracy/明确探针、跨文件rename |
| Python | import alias、动态getattr、异步队列、共享状态 | VizTracer实际采集、LibCST/语言变更、pytest |
| JS/TS | JS动态属性、TS路径别名、重载、Promise | 实际Node场景、TS语言rename、构建+测试 |
| Java | 重载、接口、反射、注解生成 | 原生trace/OTLP显式span、JDT变更、构建测试 |
| C# | partial class、delegate、生成代码 | EventPipe/OTLP、Roslyn、dotnet test |
| Go | interface、goroutine、channel阻塞 | execution trace/OTLP、gopls、go test |
| Rust | trait、cfg、宏、async、FFI边界 | 实际tracing/OTLP、rust-analyzer、cargo test |

另建混合C++/Python/UI链；安全夹具包含路径穿越、symlink/junction竞态、提示注入、可执行项目配置、进程树超时；持久夹具包含数据库中断、损坏blob、被引用artifact的GC、旧schema升级。

真实中型项目由B001/B011–B014选择固定提交，记录许可证、获取方式、最少目标子集、工具链与测试命令，不要求把大量第三方源码塞入本包。没有可用真实项目时任务未完成，不能靠语言玩具自称生产支持。
