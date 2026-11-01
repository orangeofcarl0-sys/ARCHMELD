# S10 · 独立验证、行为契约与验收

## 1. 六类检查

构建/语义有效性、行为输出、状态/并发、架构约束、性能/资源、领域正确性。每个 Check 规定 kind、runner、argv、cwd、env allowlist、输入 artifacts、timeout、expected predicate、required、fixture/source versions。

测试框架并不固定为一种；适配项目已有 pytest、CTest、Jest/Vitest、Maven/Gradle、dotnet、go、cargo 等命令。`exit_code=0` 只是通用命令结果，不自动证明契约覆盖充分。验证器要记录具体报告和实际测试数量，零测试运行不得伪装为成功回归。

## 2. 结果状态

每项 check 为 passed、failed、not_run、unsupported、inconclusive、cancelled、timed_out、error。最终判定：所有 required passed、snapshot/contract/toolchain digest 对应、架构目标门槛满足且无阻塞风险，才 accepted。optional not_run 可保留，但不得显示成 passed。

baseline 和 candidate 使用相同输入/契约与可比较环境；缺 baseline 时不能自动声称无回归。失败测试修改期望需要契约审批，不能自动由执行 Agent“修正测试”。

## 3. 验证独立性

主 Agent 可提出检查，但执行器按冻结 contract 运行。日志、退出码、测试报告、输入输出 hash 保存在 evidence。允许独立只读审查 Agent 辅助发现遗漏，但其“同意”不能替代验证。最终 verdict 由确定性 evaluator 产生，而非模型输出布尔值。

检查进程只获得需要的文件与权限；不能访问审批令牌或修改基线期望。runner 与 core 不共享可写验收配置。任何检测到的修改导致 evidence_invalid。

## 4. 行为差分

CLI/API/文件格式按规范比较；非确定时间戳等字段需要预先声明归一化规则。错误类型、取消、资源释放与重试语义也属行为。不要只比较最终成功输出。

包含内部结构变化时，图 diff 与约束 checker 检验 To-Be；运行开销通过配对场景、预热、重复次数与分布指标验证。没有测量足够重复时返回 inconclusive，而不是取最有利一次。

## 5. 数值与科研

数值契约含单位、形状、dtype、随机种子、atol/rtol 或统计判据、初始化、更新顺序与延迟。容差由任务领域定义而非工作台默认猜测。NaN/Inf、极端输入、相位 wrap、数组轴和状态复用必须单列。

R1 应含科学计算测试示例，验证算法重构保持确定性基线及动态响应；不要求工作台内置所有学科模型。详细见 S17。

## 6. 发布可证据化

每次验收保存：源码结果 snapshot、contract hash、toolchain lock hash、执行环境、命令数组、开始/结束、退出码、结果状态、产物哈希、coverage、runner isolation。artifact 没有上传远端不影响本地复核，但导出 bundle 必须包括必要 manifest。

故障注入至少包括测试超时、零测试、报告损坏、基线丢失、验证期间源码改变、模型伪造成功消息。以上全部不得进入 accepted。
