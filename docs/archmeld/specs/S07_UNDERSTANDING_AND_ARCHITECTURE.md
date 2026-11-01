# S07 · 理解引擎、架构模型与目标设计

## 1. 调查不是全文摘要

调查工作项包含 question、scope、facts、hypotheses、missing evidence、experiments、conclusions、residual risks。结论必须链接 Evidence/Claim，标明适用场景。模型不得直接写入 analysis evidence 表；只能提交 Claim、候选架构与候选计划，经格式、引用与权限检查后保存。

最小调查循环：明确问题 → 提取局部上下文 → 提出可区分的解释 → 找反例或运行实验 → 修正 → 输出带证据结论。没有证据的“很可能”不能作为自动大改理由。

## 2. 多视图共享同一实体层

提供物理结构、职责组件、执行场景、状态/资源、目标约束五类投影。Entity 可属于多个视图，不强制一棵五层树；视图节点包含 members 和 abstraction rationale，不能把目录聚类直接等同架构组件。

组件字段：purpose、responsibilities、non_responsibilities、interfaces、owned_state、owned_resources、dependencies、runtime_placement、implementation_refs、verification_refs、open_questions。数据与状态所有权是显式关系，不根据名称后缀 Manager 自动建立。

## 3. As-Is、To-Be、Transition

As-Is 描述当前实现与观察；To-Be 表达期望边界与契约；Transition 表达可暂存的适配层和迁移依赖。三者独立版本，不能拖动目标节点就直接修改现状事实。

目标图操作产生 typed intent，例如 split_responsibility、move_ownership、remove_dependency、introduce_boundary、retire_component。每项 intent 有原因、受影响实体、必须保持的契约和待验证问题。任意图操作不能绕过 change plan。

过渡例外必须绑定 change step 和终止条件；不支持没有 owner/到期条件的永久忽略规则。

## 4. 候选生成

可用 CodeBoarding 的组件/关系结果、外部 Agent、源码依赖/类型/资源访问/历史协同修改生成候选。导入记录 provider、model/config（非秘密）、输入 context digest、输出 artifact；输出只进入 candidate。[SRC-CB]

候选验证至少检查：所有源码引用真实存在且属于声明快照；职责是否存在相反实现；所谓资源所有者是否唯一；模块划分是否割裂强运行耦合。没有证据的宏观判断保持 unverified。

用户可以接受、修订或拒绝候选；接受表示该解释用于当前设计，不表示已经形式化证明。历史候选保留并可对比。

## 5. 架构约束

首版规则种类：禁止依赖、允许分层方向、无环、公共边界只能通过接口、单写入所有权声明、禁止 UI 直接读内部可变状态的适配规则。规则标明检查实现与 coverage：纯 import 规则可确定，线程所有权通常还需要运行/人工契约。

不能以“有规则文本”冒充“规则可执行”。缺少 checker 的规则标 declared_only；required declared_only 会阻止相关重构最终验收。

## 6. 重构建议的价值与反例

每条建议至少对比不改、最小调整、架构调整三种选择（简单任务可合并但必须解释）。比较行为风险、维护代价、资源成本、理解成本和验证难度；不把减少行数、增加类或降低单个复杂度指标作为唯一目标。

保留“不重构”的合法结果。数值算法中保留数学推导次序、状态更新和单位约束可能比增加抽象层更重要。策略更改、性能算法替换与纯结构性重构必须分开声明。
