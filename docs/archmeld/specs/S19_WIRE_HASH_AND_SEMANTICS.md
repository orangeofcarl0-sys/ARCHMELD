# S19 · Wire 协议、哈希与语义一致性

本文补足 JSON Schema 无法单独保证的约束。`MUST` 为验收要求。Schema 校验通过不等于业务对象可接受。

## 1. AM-C14N-1 与摘要

不自称实现 RFC 8785。采用范围受限且有测试向量的 AM-C14N-1：JSON 对象键限 ASCII；键按 Unicode 码点升序（ASCII 下等同字节排序）；无无意义空格；UTF-8 编码；字符串保持原 Unicode 序列，不 NFC/NFD 归一化，不转义非 ASCII；`"`、反斜杠和 U+0000–001F 使用 JSON 转义，其中 `\b\t\n\f\r` 用短形式，其余控制字符用小写十六进制 `\u00xx`；斜线不转义；拒绝孤立代理项；布尔/null 小写；数组原序；数值仅允许绝对值不超过 2^53−1 的整数；拒绝浮点、NaN、Infinity 和重复键。时间戳、纳秒、数值容差和测量量在需哈希的契约中以十进制字符串表达。`-0` 解析后规范为 `0`。

定义 `H(x) = "sha256:" + hex_lower(SHA256(UTF8(AM-C14N-1(x))))`。文件/blob 摘要直接针对原始字节，不经过此 JSON 规范化，不转换 CRLF。

| 对象 | 哈希输入 |
|---|---|
| Snapshot.id | 完整 Snapshot 去掉 `id`、`created_at`；files 按 path 的 UTF-8 字节序排序；同一 manifest 禁止重复 path |
| BehaviorContract.digest | 完整契约去掉 `digest`、`frozen`；已冻结与否是状态而非内容 |
| PreparedChange.digest | `{version:"1.0",project_id,plan_id,plan_revision,step_id,base_snapshot_id,contract_digest,allowed_paths,read_set,operations,toolchain_digest,staging_verification_ids}`；所有数组维持规范输入顺序，allowed_paths/read_set 按 path 排序，operations 按执行顺序；批准不能只绑定 Plan ID |
| 配置/环境/exclusions | 明确版本化的 manifest，使用 H；原始 secret 不入 manifest，使用 secret_ref 和版本标识 |
| ContextPack/报告等 | 内容摘要以最终序列化字节生成，不能把不同编码的内容误当相同对象 |

PreparedChange 的完整规范输入保存为 artifact；ChangeSet.prepared_digest 指向其 H，工具链与暂存验证不应只留在日志里。重复同一请求不生成不同的审批对象。哈希向量见 examples/hash-vectors.json。

## 2. 授权的两层语义

ChangeSet 的 `draft -> approved` 表示**方案与范围获准进入准备**，不是应用授权。此动作基于已审阅 Plan、scope 与可执行权限；prepared 后必须再次持有针对 `prepared_digest` 的 approval，才能 apply。准备和验证涉及执行代码时，同样需要 execution 权限。

PlanScope 摘要输入为 `{version:"1.0",plan:完整Plan去掉status,contract_digest,requested_capabilities}`。core在审阅界面计算并登记该manifest；target_kind=plan_scope且target_id=Plan.id的人工批准核对digest后冻结对应契约、将Plan置approved。ChangeSet创建保持draft；prepare动作在批准scope覆盖时先记录approved再进入preparing。prepared_change的target_id是ChangeSet.id，digest必须等于prepared_digest。其他审批类型以该操作完整请求、固定snapshot和限制生成登记manifest。未知target或不匹配digest拒绝，不能仅接受任意hash字符串。

UI 提供“批准方案并准备”和“应用此确切变更”两个清楚动作，后者显示差异、契约、验证情况与风险。`Approval.actor`、时间、范围由服务器从受信交互身份产生，不能由普通 Agent JSON 自报。仅人类批准端点/预先限定的策略能签发 approval；MCP 不提供 approve 工具。审批过期、契约变更、准备对象变化均失效。批准不得等同于签名不可抵赖：本地用户拥有数据库权限时不做此安全承诺。

## 3. 跨对象语义检查

每次入库或应用至少检查：

1. 引用的 project、snapshot、config 和实体身份一致；不存在跨项目拼接证据。历史 snapshot 可读取，不能以 current 冒用。
2. 源码 anchor 对应原 blob；0 ≤ start ≤ end ≤ 原始字节长度且落在 UTF-8 字符边界。line_only 不伪造 byte 精度。不可解码源文件保留字节 artifact，标记 unsupported encoding。
3. 同一批次序号从 0 单调增加；同 run+sequence 重传内容一致则幂等，不一致则冲突。final 后不接收新批次；缺页或取消不得发布为完整结果。
4. EvidenceBatch 的所有入库实体、关系和 evidence 与批次 scope 一致；关系端点必须存在于本批/同快照已有实体中；evidence 引用必须存在；resolved_call 至少有 semantic 证据，observed_call 至少有对应绑定运行证据。LLM 文字不能补足。
5. Coverage 的计数定义：observed 是已产生结果的输入项；omitted 是有意未处理；unresolved 是已处理但无法完成解析的输入项，可以与 observed 重叠。expected 已知时 observed+omitted≤expected；complete 要求 expected 已知、observed=expected、omitted=0、unresolved=0。完整仅针对声明 scope，不代表运行路径全覆盖。
6. claim 支持/反例引用存在；目标规则只由人工声明也可以合法存在，但不冒称已实现；数据变动使受影响 claim needs_review。
7. Plan steps 的 ID 唯一且依赖无环；所有 required_check_ids 在冻结契约存在；scope 不能大于已批准路径。变更语义与 Plan.kind 一致。
8. 每个检查必须恰好选择 command 或内置 checker_id 之一；不能二者皆空。执行前记录完整 runner/toolchain digest。
9. Verification 的 checks 必须与冻结契约一一匹配：缺少必需项、重复 check_id、修改 required 标记、使用不同契约/快照/工具链都不能 accepted。不存在“空测试集通过”。required 检查任何非 passed 结果都禁止验收。
10. runtime binding=bound 必须与 manifest/build/config 相符；仅给定 snapshot_id 不足以绑定。运行文件覆盖不完整时保留 partial，不能推广为全项目等价。
11. 任何 apply/recover 操作均需重新检查路径链接、权限、租约和内容 hash；失效 token 不恢复写权限；Agent 传入的状态/frozen/accepted 字段不直接采信。
12. 更新使用 expected_revision 乐观锁；幂等键作用域是 project+operation，保存请求 digest 和最终结果；同键不同 payload 返回 scope_conflict。

## 4. 文件操作规则

| op | before | after/content | destination |
|---|---|---|---|
| create | null；目标必须不存在 | 非 null，content 原字节 hash=after | null |
| update | 非 null；当前相符 | 非 null，content hash=after | null |
| delete | 非 null；当前相符 | 均 null | null |
| move | 非 null；源相符 | after=before，content 可 null | 必须是不同、已授权且不存在的目标路径 |

第一版 move 不同时改内容；需要改内容时拆为 move 后 update，并固定执行顺序。大小写重命名用平台专门测试的临时路径流程，记录每个步骤，不依赖大小写折叠的字符串判断。原地文件类型改变、链接、设备文件、Git 内部目录默认拒绝；二进制修改第一版仅在明示 artifact 与人工批准下允许。

## 5. Predicate 参数

Predicate.parameters 虽是扩展对象，第一版按 kind 拒绝未知必需语义：

| kind | 必需参数及判断 |
|---|---|
| exit_code | expected_codes: 非空整数数组；仅检查构建/命令，不冒充完整测试 |
| test_report | format:junit 或约定 JSON，artifact_role，min_tests≥1，max_failures=0；测试进程退出码也须为0 |
| file_digest | path 与 expected_digest；路径属于固定 snapshot |
| numeric_tolerance | expected_artifact、actual_artifact、columns、atol/rtol十进制字符串、nonfinite_policy:reject/match；形状/单位先检查 |
| architecture_rules | architecture_id、revision、rule_ids；required 规则必须 executable 且全覆盖 |
| invariant | checker_id、version、configuration；未注册 checker 报 unsupported，不让 LLM 自判 |
| performance_budget | metric、statistic、threshold十进制字符串、unit、comparison、min_repetitions、environment_digest；未满足重复与环境要求报 inconclusive |

具体 runner/report parser 在 B022 提供并纳入同一锁定；不存在的阈值或计量单位不能临时猜测。

## 6. 事件 payload

事件为通知，不是独立事实存储。客户端收到后根据 object ID 拉取权威对象；使用 project 级持久递增 sequence，十进制字符串，断线可续传；游标被压缩后返回 resync_required。

| type | payload 必需字段 |
|---|---|
| snapshot.created | snapshot_id, previous_snapshot_id|null, project_revision |
| analysis.progress | run_id, processed_count, expected_count|null |
| analysis.completed / failed | run_id, coverage, diagnostic_ids[] |
| claim.invalidated | claim_ids[], reason, previous_snapshot_id |
| change.state_changed | changeset_id, previous_state, state, revision |
| verification.completed | verification_id, verdict |
| job.state_changed | job_id, previous_state, state |
| instigator.nudge | task_id, task_revision, expires_at, reason, existing_scope_ref |

所有 ID 必须存在/对应 project；payload 不携带 secret 或源码全文。无消费者时不阻塞 job。顺序号不要用 JavaScript number。

## 7. 路径、HTTP 和错误

外部请求只用 workspace_handle/import_handle，受信本地 CLI/扩展通过系统选择器和权限层创建；这不是可任意拼接的绝对路径。任何 API 请求都不能凭一个字符串读整个磁盘。报表下载通过已授权 artifact handle；MCP 返回摘要/引用，原始大对象不塞入模型。

HTTP：invalid_request=400；permission_denied=403；not_found=404；stale_snapshot/scope_conflict/recovery_required=409；unsupported=422；resource_exhausted=429；tool_unavailable=503；internal_error=500。长任务通常返回202 Job，验证失败通常是 Job/Verification 的结果而非 HTTP 服务故障。API 既不能以 HTTP200 模糊失败，也不能让正常未通过的测试触发无限网络重试。

默认最大 JSON 请求16MiB、单批500实体/2000关系、单原始 artifact 由 profile 单独限额；更大输入以授权文件导入并流式解析。超限返回明确错误；分页结果标明截断。契约主版本不匹配拒绝，未知可选能力返回 unsupported，不静默忽略。
