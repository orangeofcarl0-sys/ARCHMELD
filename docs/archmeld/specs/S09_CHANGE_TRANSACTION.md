# S09 · 计划、变更事务、授权与恢复

## 1. 三种变更性质

structural_refactor 保持已声明行为；behavior_fix 显式改变已确认缺陷；architecture_migration 可改变接口或部署但需兼容策略。一个项目可包含三类，单个 ChangeSet 必须说明类型，不在“纯重构”中偷偷改变算法或协议。

Plan 保存 rationale、As-Is/To-Be、non-goals、steps DAG、风险和验收契约。ChangeSet 是一个可应用的有限变更单元，绑定 plan step、基础 snapshot、contract_digest、allowed_paths/symbols、read_set、write_set、工具与 patch artifact。

## 2. 冻结行为契约

契约包括输入/输出/错误/生命周期/状态/性能/领域要求，以及 required 检查与明确阈值。契约哈希绑定审批和 verification。修改期望、扩大容差、删测试或改 must-run 列表会使原批准与验证失效，必须作为单独契约变更处理。

已知旧缺陷明确记录 known_defect，不自动提升为正确行为。与本次修复无关的旧缺陷可保留，但不能借此忽略新失败。

## 3. 准备阶段

先验证基础快照、读取集与目标路径。把需要的源码和构建上下文物化到 session 暂存目录，不使用默认多 Git worktree，也不在真实项目里让多个 Agent 并发编辑。Agent/原生变换工具只写暂存区；所有生成文件和额外改动要纳入 diff。

原生 rename/refactor 优先，其次结构化 codemod，最后受限 LLM patch。LSP WorkspaceEdit 只接收明确 text/file 操作；任意 workspace/executeCommand 不自动执行。重命名大小写、文件创建/删除、符号链接和二进制修改需专门规则。

暂存验证通过后生成 prepared ChangeSet：完整前后内容 hash、原始字节备份、确切 patch、检查证据、工具 identity、scope digest。批准绑定该 digest，不能批准泛泛一句“重构项目”。

## 4. 应用与并发

获取项目内部单写入租约并校验 owner token；这不能阻止用户编辑器/外部工具写入，因此每个文件仍需 compare-and-swap 检查。默认任何相关 read/config snapshot 变化使 ChangeSet stale；允许重新准备/验证，不能强行套用旧 patch。

先持久化 apply journal 和所有 preimage，再逐文件执行临时文件写入、flush、受支持的替换；禁止声称跨文件操作原子。路径在应用时重新做根目录/链接检查，防止准备到应用间 TOCTOU。apply 中断进入 recovery_required/failed，不显示已完成。

应用后捕获 resulting_snapshot，重新运行 required 验证与架构 diff；只有对应结果和契约一致才 accepted。预先 staging 测试不能代替应用后来源确认与必要 smoke test。

## 5. 状态机

`draft -> approved -> preparing -> prepared -> applying -> applied -> verifying -> accepted`。

其中 approved 只授权准备，prepared 之后应用还必须持有绑定 prepared_digest 的确切变更批准，详见 S19。

批准/准备后输入变动可转 stale；拒绝转 rejected；验证失败转 failed；应用/恢复冲突转 conflicted；恢复为 recovering -> recovered。详细合法迁移见 contracts/state-machines.json。客户端不能直接写状态；由 core 根据动作和证据判定。

## 6. 恢复规则

恢复只操作本 ChangeSet 拥有的写入。对每个文件比较当前 hash 与本次 postimage：一致才可还原 preimage；若用户已再次编辑则报告 conflict，保留两份内容并提示三方合并，不覆盖。

新建文件只在内容仍与 postimage 相同且无第三方改动时删除。删除/移动文件保存原路径、权限与备份；恢复失败留可重试 journal。不得使用全仓 hard reset/clean。外部数据库、硬件、服务副作用不由文件回滚覆盖，默认在隔离场景执行；外部迁移需要单独计划。

## 7. 必测不变量

未批准不能 apply；过期不能 apply；只读工具不能写；不越权路径；原有脏文件不丢；注入第 N 个文件故障可恢复；中途编辑保留冲突；契约变更使批准失效；重复请求幂等；租约过期不允许旧 owner 继续写。接收 arbitrary shell 不是变更协议。
